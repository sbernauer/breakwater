use std::{cell::UnsafeCell, slice};

// We know that users can only supply 4 digits as coordinates, so up to 9999
// As we make the framebuffer bigger we can eliminate the bound checks
// In case the users users coordinates something bigger than framebuffer size, we write it to memory
// residing outside of the official framebuffer.
//
// By using a power of two we can bitshift instead of doing a multiplication with the width (or height)
//
// This consumes more memory, but should be worth it
const INTERNAL_FRAMEBUFFER_SIZE_MULTIPLE_OF_TWO: u32 = 14;
const INTERNAL_FRAMEBUFFER_SIZE: usize = 2_usize.pow(INTERNAL_FRAMEBUFFER_SIZE_MULTIPLE_OF_TWO);

pub struct FrameBuffer {
    width: usize,
    height: usize,
    buffer: UnsafeCell<Vec<u32>>,
}

// FIXME Nothing to see here, I don't know what I'm doing ¯\_(ツ)_/¯
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(INTERNAL_FRAMEBUFFER_SIZE.pow(2));
        buffer.resize_with(buffer.capacity(), || 0);
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
            unsafe {
                Some((*self.buffer.get())[x + (y << INTERNAL_FRAMEBUFFER_SIZE_MULTIPLE_OF_TWO)])
            }
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        unsafe { (*self.buffer.get())[x + (y << INTERNAL_FRAMEBUFFER_SIZE_MULTIPLE_OF_TWO)] }
    }

    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, rgba: u32) {
        unsafe { (*self.buffer.get())[x + (y << INTERNAL_FRAMEBUFFER_SIZE_MULTIPLE_OF_TWO)] = rgba }
    }

    pub fn get_buffer(&self) -> *mut Vec<u32> {
        // TODO: rewrite for oversized framebuffer
        self.buffer.get()
    }

    pub fn as_bytes(&self) -> &[u8] {
        // TODO: rewrite for oversized framebuffer
        let buffer = self.buffer.get();
        let len_in_bytes: usize = unsafe { (*buffer).len() } * 4;

        unsafe { slice::from_raw_parts((*buffer).as_ptr() as *const u8, len_in_bytes) }
    }
}
