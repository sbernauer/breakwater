use std::{cell::UnsafeCell, slice};

pub struct FrameBuffer {
    width: usize,
    height: usize,
    buffer: UnsafeCell<Vec<u32>>,
}

// FIXME Nothing to see here, I don't know what I'm doing ¯\_(ツ)_/¯
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, || 0);
        FrameBuffer {
            width,
            height,
            buffer: UnsafeCell::from(buffer),
        }
    }

    pub fn get_width(&self) -> usize {
        self.width
    }

    pub fn get_height(&self) -> usize {
        self.height
    }

    pub fn get_size(&self) -> usize {
        self.width * self.height
    }

    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> Option<u32> {
        if x < self.width && y < self.height {
            unsafe { Some((*self.buffer.get())[x + y * self.width]) }
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, rgba: u32) {
        // TODO: If we make the FrameBuffer large enough (e.g. 10_000 x 10_000) we don't need to check the bounds here (x and y are max 4 digit numbers).
        // (flamegraph has shown 5.21% of runtime in this bound check O.o)
        if x < self.width && y < self.height {
            unsafe { (*self.buffer.get())[x + y * self.width] = rgba }
        }
    }

    pub fn get_buffer(&self) -> *mut Vec<u32> {
        self.buffer.get()
    }

    pub fn as_bytes(&self) -> &[u8] {
        let buffer = self.buffer.get();
        let len_in_bytes: usize = unsafe { (*buffer).len() } * 4;

        unsafe { slice::from_raw_parts((*buffer).as_ptr() as *const u8, len_in_bytes) }
    }
}
