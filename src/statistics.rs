use std::collections::HashMap;
use std::fs::File;
use std::net::IpAddr;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use prometheus::core::{AtomicI64, GenericGauge, GenericGaugeVec};
use prometheus::{register_int_gauge, register_int_gauge_vec};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Statistics {
    #[serde(skip)]
    pub save_file: Option<String>,

    // These statistics are always up to date
    #[serde(skip)]
    connections_for_ip: Mutex<HashMap<IpAddr, AtomicU32>>,
    bytes_for_ip: Mutex<HashMap<IpAddr, AtomicU64>>,
    pixels_for_ip: Mutex<HashMap<IpAddr, AtomicU64>>,
    #[serde(skip)]
    pub frame: AtomicU64,

    // Whether the current stats have been printed to the screen.
    // With this we can only draw the stats one - directly after updating them - and not every frame
    // By doing so we avoid flickering stats and don't need to mark the rect as modified every frame besides nothing having changed
    #[serde(skip)]
    pub stats_on_screen_are_up_to_date: AtomicBool,

    // Variables to hold the statistics at the last time gathered
    #[serde(skip)]
    pub current_connections: AtomicU32,
    #[serde(skip)]
    pub current_ips: AtomicU32,
    #[serde(skip)]
    pub current_legacy_ips: AtomicU32,
    #[serde(skip)]
    pub current_bytes: AtomicU64,
    #[serde(skip)]
    pub current_pixels: AtomicU64,
    #[serde(skip)]
    pub current_frame: AtomicU64,

    #[serde(skip)]
    pub bytes_per_s: AtomicU64,
    #[serde(skip)]
    pub pixels_per_s: AtomicU64,
    #[serde(skip)]
    pub fps: AtomicU64,

    // Prometheus metrics
    #[serde(skip)]
    metric_connections: GenericGaugeVec<AtomicI64>,
    #[serde(skip)]
    metric_ips: GenericGauge<AtomicI64>,
    #[serde(skip)]
    metric_legacy_ips: GenericGauge<AtomicI64>,
    #[serde(skip)]
    metric_bytes: GenericGaugeVec<AtomicI64>,
    #[serde(skip)]
    metric_pixels: GenericGaugeVec<AtomicI64>,
    #[serde(skip)]
    metric_fps: GenericGauge<AtomicI64>,
}

impl Statistics {
    pub fn new(save_file: Option<&str>) -> Self {
        Statistics {
            save_file: save_file.map(str::to_string),

            connections_for_ip: Mutex::new(HashMap::new()),
            bytes_for_ip: Mutex::new(HashMap::new()),
            pixels_for_ip: Mutex::new(HashMap::new()),
            frame: AtomicU64::new(0),

            stats_on_screen_are_up_to_date: AtomicBool::new(false),

            current_connections: AtomicU32::new(0),
            current_ips: AtomicU32::new(0),
            current_legacy_ips: AtomicU32::new(0),
            current_bytes: AtomicU64::new(0),
            current_pixels: AtomicU64::new(0),
            current_frame: AtomicU64::new(0),

            bytes_per_s: AtomicU64::new(0),
            pixels_per_s: AtomicU64::new(0),
            fps: AtomicU64::new(0),

            metric_connections: register_int_gauge_vec!(
                "breakwater_connections",
                "Number of client connections",
                &["ip"]
            )
            .unwrap(),
            metric_ips: register_int_gauge!("breakwater_ips", "Number of IPs connected").unwrap(),
            metric_legacy_ips: register_int_gauge!(
                "breakwater_legacy_ips",
                "Number of legacy (v4) IPs connected"
            )
            .unwrap(),
            metric_bytes: register_int_gauge_vec!(
                "breakwater_bytes",
                "Number of bytes received",
                &["ip"]
            )
            .unwrap(),
            metric_pixels: register_int_gauge_vec!(
                "breakwater_pixels",
                "Number of Pixels set",
                &["ip"]
            )
            .unwrap(),
            metric_fps: register_int_gauge!(
                "breakwater_fps",
                "Frames per second of the VNC server"
            )
            .unwrap(),
        }
    }

    pub fn from_save_file_or_new(save_file: &str) -> Self {
        let mut statistics = Statistics::new(Some(save_file));

        if let Ok(save_point) = StatisticsSavePoint::load(save_file) {
            statistics.bytes_for_ip = Mutex::new(save_point.bytes_for_ip);
            statistics.pixels_for_ip = Mutex::new(save_point.pixels_for_ip);
        }

        statistics
    }

    pub fn inc_connections(&self, ip: IpAddr) {
        self.connections_for_ip
            .lock()
            .unwrap()
            .entry(ip)
            .or_insert(AtomicU32::new(0))
            .fetch_add(1, AcqRel);
    }

    pub fn dec_connections(&self, ip: IpAddr) {
        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        match connections_for_ip.get(&ip) {
            None => {}
            Some(connections) => {
                let previous_connections_for_ip = connections.fetch_sub(1, AcqRel);
                if previous_connections_for_ip <= 1 {
                    connections_for_ip.remove(&ip);
                    self.metric_connections
                        .remove_label_values(&[&ip.to_string()])
                        .ok();
                }
            }
        }
    }

    fn get_connections(&self) -> u32 {
        self.connections_for_ip
            .lock()
            .unwrap()
            .values()
            .map(|i| i.load(Acquire))
            .sum()
    }

    fn get_ip_count(&self) -> u32 {
        self.connections_for_ip.lock().unwrap().len() as u32
    }

    fn get_ip_count_legacy(&self) -> u32 {
        self.connections_for_ip
            .lock()
            .unwrap()
            .keys()
            .filter(|ip| ip.is_ipv4())
            .count() as u32
    }

    #[inline(always)]
    pub fn inc_bytes(&self, ip: IpAddr, bytes: u64) {
        self.bytes_for_ip
            .lock()
            .unwrap()
            .entry(ip)
            .or_insert(AtomicU64::new(0))
            .fetch_add(bytes, AcqRel);
    }

    /// Expensive!
    /// Should only be called when feature `count_pixels` is enabled
    #[inline(always)]
    pub fn inc_pixels(&self, ip: IpAddr) {
        self.pixels_for_ip
            .lock()
            .unwrap()
            .entry(ip)
            .or_insert(AtomicU64::new(0))
            .fetch_add(1, AcqRel);
    }

    fn get_bytes(&self) -> u64 {
        self.bytes_for_ip
            .lock()
            .unwrap()
            .values()
            .map(|i| i.load(Acquire))
            .sum()
    }

    fn get_pixels(&self) -> u64 {
        self.pixels_for_ip
            .lock()
            .unwrap()
            .values()
            .map(|i| i.load(Acquire))
            .sum()
    }

    fn update(&self) {
        // Calculate statistics
        self.current_connections
            .store(self.get_connections(), Release);
        self.current_ips.store(self.get_ip_count(), Release);
        self.current_legacy_ips
            .store(self.get_ip_count_legacy(), Release);

        let new_bytes = self.get_bytes();
        self.bytes_per_s
            .store(new_bytes - self.current_bytes.load(Acquire), Release);
        self.current_bytes.store(new_bytes, Release);

        if cfg!(not(feature = "count_pixels")) {
            // Do a crude estimation if actual pixel count is not available. Average Pixel is about |PX XXX YYY rrggbb\n| = 18 bytes
            self.bytes_for_ip
                .lock()
                .unwrap()
                .iter()
                .for_each(|(ip, bytes)| {
                    self.pixels_for_ip
                        .lock()
                        .unwrap()
                        .entry(*ip)
                        .or_insert(AtomicU64::new(0))
                        .store(bytes.load(Acquire) / 18, Release)
                });
        }
        let new_pixels = self.get_pixels();
        self.pixels_per_s
            .store(new_pixels - self.current_pixels.load(Acquire), Release);
        self.current_pixels.store(new_pixels, Release);

        let new_frame = self.frame.load(Acquire);
        self.fps
            .store(new_frame - self.current_frame.load(Acquire), Release);
        self.current_frame.store(new_frame, Release);

        // Put statistics into Prometheus metrics
        self.connections_for_ip
            .lock()
            .unwrap()
            .iter()
            .for_each(|(ip, connections)| {
                self.metric_connections
                    .with_label_values(&[&ip.to_string()])
                    .set(connections.load(Acquire) as i64)
            });
        self.metric_ips.set(self.current_ips.load(Acquire) as i64);
        self.metric_legacy_ips
            .set(self.current_legacy_ips.load(Acquire) as i64);
        self.bytes_for_ip
            .lock()
            .unwrap()
            .iter()
            .for_each(|(ip, bytes)| {
                self.metric_bytes
                    .with_label_values(&[&ip.to_string()])
                    .set(bytes.load(Acquire) as i64)
            });
        self.pixels_for_ip
            .lock()
            .unwrap()
            .iter()
            .for_each(|(ip, pixels)| {
                self.metric_pixels
                    .with_label_values(&[&ip.to_string()])
                    .set(pixels.load(Acquire) as i64)
            });
        self.metric_fps.set(self.fps.load(Acquire) as i64);

        // Force re-draw of stats on screen
        self.stats_on_screen_are_up_to_date
            .store(false, Ordering::SeqCst);
    }

    /// Saves the statistics to the save-file if enabled
    pub fn save_to_save_file(&self) {
        if let Some(save_file) = &self.save_file {
            match File::create(save_file) {
                Ok(file) => match serde_json::to_writer(file, &self) {
                    Ok(()) => (),
                    Err(err) => {
                        println!("Failed to write to statistics save file {save_file}: {err}")
                    }
                },
                Err(err) => println!("Failed to create statistics save file {save_file}: {err}"),
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct StatisticsSavePoint {
    pub bytes_for_ip: HashMap<IpAddr, AtomicU64>,
    pub pixels_for_ip: HashMap<IpAddr, AtomicU64>,
}

impl StatisticsSavePoint {
    pub fn load(save_file: &str) -> std::io::Result<Self> {
        let file = File::open(save_file)?;
        Ok(serde_json::from_reader(file)?)
    }
}

pub fn start_loop(statistics: Arc<Statistics>, save_interval_s: u64) {
    let statistics_1 = Arc::clone(&statistics);
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        statistics_1.update();
    });

    if statistics.save_file.is_some() {
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(save_interval_s));
            statistics.save_to_save_file();
        });
    }
}

pub fn start_prometheus_server(prometheus_listen_address: &str) {
    prometheus_exporter::start(prometheus_listen_address.parse().unwrap_or_else(|_| {
        panic!(
            "Failed to parse prometheus listen address: {}",
            prometheus_listen_address
        )
    }))
    .expect("Failed to start prometheus exporter");
    println!("Started Prometheus Exporter on {prometheus_listen_address}");
}
