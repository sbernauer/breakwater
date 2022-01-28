use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::atomic::Ordering::Relaxed;
use std::thread;
use std::time::Duration;

use prometheus::{register_int_gauge, register_int_gauge_vec};
use prometheus::core::{AtomicI64, GenericGauge, GenericGaugeVec};

pub struct Statistics {
    connections_for_ip: Mutex<HashMap<IpAddr, AtomicU32>>,
    bytes_for_ip: Mutex<HashMap<IpAddr, AtomicU64>>,

    pub current_connections: AtomicU32,
    pub current_ips: AtomicU32,
    pub current_legacy_ips: AtomicU32,
    pub current_bytes: AtomicU64,

    pub bytes_per_s: AtomicU64,

    metric_connections: GenericGaugeVec<AtomicI64>,
    metric_ips: GenericGauge<AtomicI64>,
    metric_legacy_ips: GenericGauge<AtomicI64>,
    metric_bytes: GenericGaugeVec<AtomicI64>,
}

impl Statistics {
    pub fn new() -> Self {
        Statistics {
            connections_for_ip: Mutex::new(HashMap::new()),
            bytes_for_ip: Mutex::new(HashMap::new()),

            current_connections: AtomicU32::new(0),
            current_ips: AtomicU32::new(0),
            current_legacy_ips: AtomicU32::new(0),
            current_bytes: AtomicU64::new(0),

            bytes_per_s: AtomicU64::new(0),

            metric_connections: register_int_gauge_vec!("breakwater_connections", "Number of client connections", &["ip"]).unwrap(),
            metric_ips: register_int_gauge!("breakwater_ips", "Number of IPs connected").unwrap(),
            metric_legacy_ips: register_int_gauge!("breakwater_legacy_ips", "Number of legacy (v4) IPs connected").unwrap(),
            metric_bytes: register_int_gauge_vec!("breakwater_bytes", "Number of bytes received", &["ip"]).unwrap(),
        }
    }

    pub fn inc_connections(&self, ip: IpAddr) {
        // Initialize connection counter
        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        connections_for_ip.entry(ip).or_insert(AtomicU32::new(0));

        // Initialize byte counter
        let mut bytes_for_ip = self.bytes_for_ip.lock().unwrap();
        bytes_for_ip.entry(ip).or_insert(AtomicU64::new(0));

        connections_for_ip[&ip].fetch_add(1, Relaxed);
    }

    pub fn dec_connections(&self, ip: IpAddr) {
        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        let remaining_connections_for_ip = connections_for_ip[&ip].fetch_sub(1, Relaxed);
        if remaining_connections_for_ip <= 1 { // We check for 1 instead of 1 as we get the value before decrementing
            connections_for_ip.remove(&ip);
            self.metric_connections.remove_label_values(&[&ip.to_string()]).ok();
        }
    }

    fn get_connections(&self) -> u32 {
        self.connections_for_ip.lock().unwrap()
            .values()
            .map(|i| i.load(Relaxed))
            .sum()
    }

    fn get_ip_count(&self) -> u32 {
        self.connections_for_ip.lock().unwrap().len() as u32
    }

    fn get_ip_count_legacy(&self) -> u32 {
        self.connections_for_ip.lock().unwrap().keys()
            .filter(|ip| ip.is_ipv4() || (ip.is_ipv6() && is_mapped_to_ipv6(ip)))
            .count() as u32
    }

    pub fn inc_bytes(&self, ip: IpAddr, bytes: u64) {
        // We dont have to check if the entry exists, as inc_connections() will create it for us
        let bytes_for_ip = self.bytes_for_ip.lock().unwrap();
        bytes_for_ip[&ip].fetch_add(bytes, Relaxed);
    }

    fn get_bytes(&self) -> u64 {
        self.bytes_for_ip.lock().unwrap()
            .values()
            .map(|i| i.load(Relaxed))
            .sum()
    }

    fn update(&self) {
        self.current_connections.store(self.get_connections(), Relaxed);
        self.current_ips.store(self.get_ip_count(), Relaxed);
        self.current_legacy_ips.store(self.get_ip_count_legacy(), Relaxed);

        let new_bytes = self.get_bytes();
        self.bytes_per_s.store(new_bytes - self.current_bytes.load(Relaxed), Relaxed);
        self.current_bytes.store(new_bytes, Relaxed);


        self.connections_for_ip.lock().unwrap().iter()
            .for_each(|(ip, bytes)|
                self.metric_connections.with_label_values(&[&ip.to_string()]).set(bytes.load(Relaxed) as i64));
        self.metric_ips.set(self.current_ips.load(Relaxed) as i64);
        self.metric_legacy_ips.set(self.current_legacy_ips.load(Relaxed) as i64);

        self.bytes_for_ip.lock().unwrap().iter()
            .for_each(|(ip, bytes)|
                self.metric_bytes.with_label_values(&[&ip.to_string()]).set(bytes.load(Relaxed) as i64));
    }
}

pub fn start_loop(statistics: Arc<Statistics>) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            statistics.update();
        }
    });
}

pub fn start_prometheus_server(prometheus_listen_address: &str) {
    prometheus_exporter::start(prometheus_listen_address.parse()
        .expect(format!("Cannot parse prometheus listen address: {prometheus_listen_address}").as_str()))
        .expect("Cannot start prometheus exporter");
    println!("Started Prometheus Exporter on {prometheus_listen_address}");
}

fn is_mapped_to_ipv6(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(ip) => match ip.segments() {
            // 5 * 16 `0` bits, 16 `1` bits, leftover is actual IPv4 addr
            [0, 0, 0, 0, 0, 0xFFFF, ..] => true,
            _ => false,
        },
        _ => false,
    }
}
