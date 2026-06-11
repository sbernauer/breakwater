pub mod shared_memory;
pub mod simple;
#[cfg(feature = "time-tracking")]
pub mod time_tracking;

pub const FB_BYTES_PER_PIXEL: usize = std::mem::size_of::<u32>();

pub trait FrameBuffer {
    /// Width in pixels
    fn get_width(&self) -> usize;

    /// Height in pixels
    fn get_height(&self) -> usize;

    /// Returns the number of pixels (not bytes)
    #[inline(always)]
    fn get_size(&self) -> usize {
        self.get_width() * self.get_height()
    }

    /// In case the coordinates are within the framebuffers area, [`Some`] with
    /// the current color is returned, [`None`] otherwise.
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

    /// As the pixel memory doesn't necessarily need to be aligned (think of using shared memory for
    /// that), we can only return it as a list of bytes, not a list of pixels.
    fn as_bytes(&self) -> &[u8];

    /// Encode `ns_since_unix_epoch` into the compact, per-pixel `coarse_ns_since_base`
    /// representation. This is the only place that touches the (atomic) base timestamp, so call
    /// it **once per parse call** and pass the result to [`Self::set_with_coarse_ns_since_base`]
    /// for every pixel. That keeps the base load (and the encoding arithmetic) out of the hot
    /// per-pixel path, where it would otherwise re-run on every single write.
    #[cfg(feature = "time-tracking")]
    fn coarse_ns_since_base(&self, ns_since_unix_epoch: u64) -> u32;

    /// Like [`Self::set`], but also records *when* the pixel was written, as the precomputed
    /// `coarse_ns_since_base` from [`Self::coarse_ns_since_base`].
    #[cfg(feature = "time-tracking")]
    fn set_with_coarse_ns_since_base(
        &self,
        x: usize,
        y: usize,
        rgba: u32,
        coarse_ns_since_base: u32,
    );
}
