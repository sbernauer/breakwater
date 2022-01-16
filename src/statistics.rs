use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Mutex;

pub struct Statistics {
    connections: AtomicU32,
    connections_for_ip: Mutex<HashMap<IpAddr, AtomicU32>>,
}

impl Statistics {
    pub fn new() -> Self {
        Statistics {
            connections: AtomicU32::new(0),
            connections_for_ip: Mutex::new(HashMap::new()),
        }
    }

    pub fn inc_connections(&self, ip: IpAddr) {
        self.connections.fetch_add(1, Relaxed);

        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        connections_for_ip.entry(ip).or_insert(AtomicU32::new(0));
        connections_for_ip[&ip].fetch_add(1, Relaxed);
    }

    pub fn dec_connections(&self, ip: IpAddr) {
        self.connections.fetch_sub(1, Relaxed);

        let mut connections_for_ip = self.connections_for_ip.lock().unwrap();
        let remaining_connections_for_ip = connections_for_ip[&ip].fetch_sub(1, Relaxed);
        if remaining_connections_for_ip <= 1 { // We check for 1 instead of 1 as we get the value before decrementing
            connections_for_ip.remove(&ip);
        }
    }

    pub fn get_connections(&self) -> u32 {
        self.connections.load(Relaxed)
    }

    pub fn get_ip_count(&self) -> u32 {
        self.connections_for_ip.lock().unwrap().len() as u32
    }
}
