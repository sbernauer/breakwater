use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;

pub struct FrameBuffer {
    pub width: usize,
    pub height: usize,
    _vec: Vec<AtomicU32>,
    slice: &'static mut [AtomicU32],
}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut vec = Vec::with_capacity(width * height);
        vec.resize_with(width * height, || AtomicU32::new(0));
        let ptr = vec.as_mut_ptr();
        unsafe {
            FrameBuffer {
                width,
                height,
                _vec: vec,
                slice: std::slice::from_raw_parts_mut(ptr, width * height),
            }
        }
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        if x < self.width && y < self.height {
            self.slice[x + y * self.width].load(Relaxed)
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, val: u32) {
        if x < self.width && y < self.height {
            self.slice[x + y * self.width].store(val, Relaxed);
        }
    }
}
