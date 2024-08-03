pub mod simple;

pub trait FrameBuffer {
    fn get_width(&self) -> usize;

    fn get_height(&self) -> usize;

    fn get_size(&self) -> usize {
        self.get_width() * self.get_height()
    }

    #[inline]
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

    fn as_bytes(&self) -> &[u8];
}
