use core::slice;

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

    #[inline(always)]
    fn set_multi_from_start_index(&self, starting_index: usize, pixels: &[u8]) -> usize {
        let num_pixels = pixels.len() / 4;

        if starting_index + num_pixels > self.buffer.len() {
            dbg!(
                "Ignoring invalid set_multi call, which would exceed the screen",
                starting_index,
                num_pixels,
                self.buffer.len()
            );
            // We did not move
            return 0;
        }

        let starting_ptr = unsafe { self.buffer.as_ptr().add(starting_index) };
        let target_slice =
            unsafe { slice::from_raw_parts_mut(starting_ptr as *mut u8, pixels.len()) };
        target_slice.copy_from_slice(pixels);

        num_pixels
    }

    #[inline(always)]
    fn as_bytes(&self) -> &[u8] {
        let len = 4 * self.buffer.len();
        let ptr = self.buffer.as_ptr() as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }

    #[inline(always)]
    fn as_pixels(&self) -> &[u32] {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rstest::{fixture, rstest};

    #[fixture]
    fn fb() -> SimpleFrameBuffer {
        // We keep the framebuffer so small, so that we can easily test all pixels in a test run
        SimpleFrameBuffer::new(640, 480)
    }

    #[rstest]
    #[case(0, 0, 0)]
    #[case(0, 0, 0xff0000)]
    #[case(0, 0, 0x0000ff)]
    #[case(0, 0, 0x12345678)]
    pub fn test_roundtrip(
        fb: SimpleFrameBuffer,
        #[case] x: usize,
        #[case] y: usize,
        #[case] rgba: u32,
    ) {
        fb.set(x, y, rgba);
        assert_eq!(fb.get(x, y), Some(rgba));
    }

    #[rstest]
    pub fn test_out_of_bounds(fb: SimpleFrameBuffer) {
        assert_eq!(fb.get(usize::MAX, usize::MAX), None);
        assert_eq!(fb.get(usize::MAX, usize::MAX), None);
    }

    #[rstest]
    pub fn test_set_multi_from_beginning(fb: SimpleFrameBuffer) {
        let pixels = (0..10_u32).collect::<Vec<_>>();
        let pixel_bytes: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();

        let (current_x, current_y) = fb.set_multi(0, 0, &pixel_bytes);

        assert_eq!(current_x, 10);
        assert_eq!(current_y, 0);

        for x in 0..10 {
            assert_eq!(fb.get(x as usize, 0), Some(x), "Checking pixel {x}");
        }

        // The next pixel must not have been colored
        assert_eq!(fb.get(11, 0), Some(0));
    }

    #[rstest]
    pub fn test_set_multi_in_the_middle(fb: SimpleFrameBuffer) {
        let mut x = 10;
        let mut y = 100;

        // Let's color exactly 3 lines and 42 pixels
        let pixels = (0..3 * fb.width as u32 + 42).collect::<Vec<_>>();
        let pixel_bytes: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        let (current_x, current_y) = fb.set_multi(x, y, &pixel_bytes);

        assert_eq!(current_x, 52);
        assert_eq!(current_y, 103);

        // Let's check everything has been colored
        for rgba in 0..3 * fb.width as u32 + 42 {
            assert_eq!(fb.get(x, y), Some(rgba));

            x += 1;
            if x >= fb.width {
                x = 0;
                y += 1;
            }
        }

        // Everything afterwards must have not been touched (let's check the next 10 lines)
        for _ in 0..10 * fb.width as u32 {
            assert_eq!(fb.get(x, y), Some(0));

            x += 1;
            if x >= fb.width {
                x = 0;
                y += 1;
            }
        }
    }

    #[rstest]
    pub fn test_set_multi_does_nothing_when_too_long(fb: SimpleFrameBuffer) {
        let mut too_long = Vec::with_capacity(fb.width * fb.height * 4 /* pixels per byte */);
        too_long.fill_with(|| 42_u8);
        let (current_x, current_y) = fb.set_multi(1, 0, &too_long);

        // Should be unchanged
        assert_eq!(current_x, 1);
        assert_eq!(current_y, 0);

        for x in 0..fb.width {
            for y in 0..fb.height {
                assert_eq!(fb.get(x, y), Some(0));
            }
        }
    }
}
