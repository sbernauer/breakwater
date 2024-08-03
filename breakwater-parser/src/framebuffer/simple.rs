use super::FrameBuffer;

pub struct SimpleFrameBuffer {
    width: usize,
    height: usize,
    buffer: Vec<u32>,
}

impl SimpleFrameBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, || 0);
        Self {
            width,
            height,
            buffer,
        }
    }
}

impl FrameBuffer for SimpleFrameBuffer {
    #[inline(always)]
    fn get_width(&self) -> usize {
        self.width
    }

    #[inline(always)]
    fn get_height(&self) -> usize {
        self.height
    }

    #[inline(always)]
    unsafe fn get_unchecked(&self, x: usize, y: usize) -> u32 {
        *self.buffer.get_unchecked(x + y * self.width)
    }

    #[inline(always)]
    fn set(&self, x: usize, y: usize, rgba: u32) {
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

    fn as_bytes(&self) -> &[u8] {
        let len = 4 * self.buffer.len();
        let ptr = self.buffer.as_ptr() as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}
