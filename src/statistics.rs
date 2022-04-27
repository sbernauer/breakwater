use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use prometheus::core::{AtomicI64, GenericGauge, GenericGaugeVec};
use prometheus::{register_int_gauge, register_int_gauge_vec};

pub struct Statistics {
    // These statistics are always up to date
    connections_for_ip: Mutex<HashMap<IpAddr, AtomicU32>>,
    bytes_for_ip: Mutex<HashMap<IpAddr, AtomicU64>>,
    pixels_for_ip: Mutex<HashMap<IpAddr, AtomicU64>>,
    pub frame: AtomicU64,

    // Whether the current stats have been printed to the screen.
    // With this we can only draw the stats one - directly after updating them - and not every frame
    // By doing so we avoid flickering stats and don't need to mark the rect as modified every frame besides nothing having changed
    pub stats_on_screen_are_up_to_date: AtomicBool,

    // Variables to hold the statistics at the last time gathered
    pub current_connections: AtomicU32,
    pub current_ips: AtomicU32,
    pub current_legacy_ips: AtomicU32,
    pub current_bytes: AtomicU64,
    pub current_pixels: AtomicU64,
    pub current_frame: AtomicU64,

    pub bytes_per_s: AtomicU64,
    pub pixels_per_s: AtomicU64,
    pub fps: AtomicU64,

    // Prometheus metrics
    metric_connections: GenericGaugeVec<AtomicI64>,
    metric_ips: GenericGauge<AtomicI64>,
    metric_legacy_ips: GenericGauge<AtomicI64>,
    metric_bytes: GenericGaugeVec<AtomicI64>,
    metric_pixels: GenericGaugeVec<AtomicI64>,
    metric_fps: GenericGauge<AtomicI64>,
}

impl Statistics {
    pub fn new() -> Self {
        Statistics {
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

    pub fn inc_connections(&self, ip: IpAddr) {
        // Initialize connection counter
        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        connections_for_ip.entry(ip).or_insert(AtomicU32::new(0));

        // Initialize counters for ip
        self.bytes_for_ip
            .lock()
            .unwrap()
            .entry(ip)
            .or_insert(AtomicU64::new(0));
        self.pixels_for_ip
            .lock()
            .unwrap()
            .entry(ip)
            .or_insert(AtomicU64::new(0));

        connections_for_ip[&ip].fetch_add(1, AcqRel);
    }

    pub fn dec_connections(&self, ip: IpAddr) {
        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        let remaining_connections_for_ip = connections_for_ip[&ip].fetch_sub(1, AcqRel);
        if remaining_connections_for_ip <= 1 {
            // We check for 1 instead of 1 as we get the value before decrementing
            connections_for_ip.remove(&ip);
            self.metric_connections
                .remove_label_values(&[&ip.to_string()])
                .ok();
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
            .filter(|ip| ip.is_ipv4() || (ip.is_ipv6() && is_mapped_to_ipv6(ip)))
            .count() as u32
    }

    #[inline(always)]
    pub fn inc_bytes(&self, ip: IpAddr, bytes: u64) {
        // We don't have to check if the entry exists, as inc_connections() will create it for us
        let bytes_for_ip = self.bytes_for_ip.lock().unwrap();
        bytes_for_ip[&ip].fetch_add(bytes, AcqRel);
    }

    /// Expensive!
    /// Should only be called when feature `count_pixels` is enabled
    #[inline(always)]
    pub fn inc_pixels(&self, ip: IpAddr) {
        // We don't have to check if the entry exists, as inc_connections() will create it for us
        let pixels_for_ip = self.pixels_for_ip.lock().unwrap();
        pixels_for_ip[&ip].fetch_add(1, AcqRel);
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
            let pixels_for_ip = self.pixels_for_ip.lock().unwrap();
            self.bytes_for_ip
                .lock()
                .unwrap()
                .iter()
                .for_each(|(ip, bytes)| pixels_for_ip[ip].store(bytes.load(Acquire) / 18, Release));
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
}

pub fn start_loop(statistics: Arc<Statistics>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        statistics.update();
    });
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

fn is_mapped_to_ipv6(ip: &IpAddr) -> bool {
    match ip {
        // 5 * 16 `0` bits, 16 `1` bits, leftover is actual IPv4 addr
        IpAddr::V6(ip) => matches!(ip.segments(), [0, 0, 0, 0, 0, 0xFFFF, ..]),
        _ => false,
    }
}
