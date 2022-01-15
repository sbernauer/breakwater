use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering::Relaxed;

use crate::{WIDTH, HEIGHT};

pub struct FrameBuffer {
    _vec: Vec<AtomicU16>,
    slice: &'static mut [AtomicU16],
}

impl FrameBuffer {
    pub fn new() -> Self {
        let mut vec = Vec::with_capacity(WIDTH * HEIGHT);
        vec.resize_with(WIDTH * HEIGHT, || AtomicU16::new(0));
        let ptr = vec.as_mut_ptr();
        unsafe {
            FrameBuffer {
                _vec: vec,
                slice: std::slice::from_raw_parts_mut(ptr, WIDTH * HEIGHT)
            }
        }
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> u16 {
        if x >= WIDTH || y >= HEIGHT {
            self.slice[x + y * WIDTH].load(Relaxed)
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, val: u16) {
        if x < WIDTH && y < HEIGHT {
            self.slice[x + y * WIDTH].store(val, Relaxed);
        }
    }
}