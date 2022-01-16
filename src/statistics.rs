use std::sync::atomic::AtomicU32;

pub struct Statistics {
    pub connections: AtomicU32,
}

impl Statistics {
    pub fn new() -> Self {
        Statistics {
            connections: AtomicU32::new(0),
        }
    }
}