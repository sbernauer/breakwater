pub mod simple;

pub trait FrameBuffer {
    fn get_width(&self) -> usize;

    fn get_height(&self) -> usize;

    fn get_size(&self) -> usize {
        self.get_width() * self.get_height()
    }

    #[inline(always)]
    fn get(&self, x: usize, y: usize) -> Option<u32> {
        if x < self.get_width() && y < self.get_height() {
            Some(unsafe { self.get_unchecked(x, y) })
        } else {
            None
        }
    }

    /// # Safety
    /// make sure x and y are in bounds
    unsafe fn get_unchecked(&self, x: usize, y: usize) -> u32;

    fn set(&self, x: usize, y: usize, rgba: u32);

    /// We can *not* take an `&[u32]` for the pixel here, as `std::slice::from_raw_parts` requires the data to be
    /// aligned. As the data already is stored in a buffer we can not guarantee it's correctly aligned, so let's just
    /// treat the pixels as raw bytes.
    ///
    /// Returns the coordinates where we landed after filling
    #[inline(always)]
    fn set_multi(&self, start_x: usize, start_y: usize, pixels: &[u8]) -> (usize, usize) {
        let starting_index = start_x + start_y * self.get_width();
        let pixels_copied = self.set_multi_from_start_index(starting_index, pixels);

        let new_x = (start_x + pixels_copied) % self.get_width();
        let new_y = start_y + (pixels_copied / self.get_width());

        (new_x, new_y)
    }

    /// Returns the number of pixels copied
    fn set_multi_from_start_index(&self, starting_index: usize, pixels: &[u8]) -> usize;

    fn as_bytes(&self) -> &[u8];

    fn as_pixels(&self) -> &[u32];
}
