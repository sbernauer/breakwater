use std::slice;


pub struct FrameBuffer {
    width: usize,
    height: usize,
    buffer: Vec<u32>,
}

impl FrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, || 0);
        FrameBuffer {
            width,
            height,
            buffer,
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
            Some(*unsafe { self.buffer.get_unchecked(x + y * self.width) })
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        *unsafe { self.buffer.get_unchecked(x + y * self.width) }
    }

    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, rgba: u32) {
        // https://github.com/sbernauer/breakwater/pull/11
        // If we make the FrameBuffer large enough (e.g. 10_000 x 10_000) we don't need to check the bounds here
        // (x and y are max 4 digit numbers). Flamegraph has shown 5.21% of runtime in this bound check. On the other
        // hand this can increase the framebuffer size dramatically and lowers the cash locality.
        // In the end we did *not* go with this change.
        if x < self.width && y < self.height {
            unsafe {
                let ptr = self.buffer.as_ptr().add(x + y * self.width) as *mut u32;
                *ptr = rgba;
            }
        }
    }

    pub fn get_buffer(&self) -> &[u32] {
        &self.buffer
    }

    pub fn as_bytes(&self) -> &[u8] {
        let len_in_bytes = self.buffer.len() * 4;
        unsafe { slice::from_raw_parts(self.buffer.as_ptr() as *const u8, len_in_bytes) }
    }
}

impl breakwater_parser::FrameBuffer for FrameBuffer {
    fn get_width(&self) -> usize {
        self.get_width()
    }

    fn get_height(&self) -> usize {
        self.get_height()
    }

    fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        self.get_unchecked(x, y)
    }

    fn set(&self, x: usize, y: usize, rgba: u32) {
        self.set(x, y, rgba)
    }

    fn get_buffer(&self) -> &[u32] {
        self.get_buffer()
    }
}
