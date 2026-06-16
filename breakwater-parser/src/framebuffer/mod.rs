#[cfg(not(feature = "time-tracking"))]
pub mod shared_memory;
#[cfg(not(feature = "time-tracking"))]
pub mod simple;
#[cfg(feature = "time-tracking")]
pub mod time_tracking;

/// Non-time-tracking framebuffer use an [`u32`] to store the color value
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

    /// Set the pixel color.
    ///
    /// Will do nothing if the coordinates are out of bounds.
    #[cfg(not(feature = "time-tracking"))]
    fn set(&self, x: usize, y: usize, rgba: u32);

    /// Set the pixel color and track the timestamp it was set.
    ///
    /// The timestamp is an arbitrary (monotonically increasing) [`u62`] number, it's actual value
    /// is defined by the caller - e.g. qs since startup or anything else.
    /// Please note that it can be truncated, as e.g. [`time_tracking::TimeTrackingFrameBuffer`]
    /// does.
    ///
    /// Will do nothing if the coordinates are out of bounds.
    #[cfg(feature = "time-tracking")]
    fn set(&self, x: usize, y: usize, rgba: u32, ts: u64);

    /// We can *not* take an `&[u32]` for the pixel here, as `std::slice::from_raw_parts` requires the data to be
    /// aligned. As the data already is stored in a buffer we can not guarantee it's correctly aligned, so let's just
    /// treat the pixels as raw bytes.
    ///
    /// Returns the coordinates where we landed after filling
    #[cfg(not(feature = "time-tracking"))] // We don't have any timestamp knowledge in this case
    #[inline(always)]
    fn set_multi(&self, start_x: usize, start_y: usize, pixels: &[u8]) -> (usize, usize) {
        let starting_index = start_x + start_y * self.get_width();
        let pixels_copied = self.set_multi_from_start_index(starting_index, pixels);

        let new_x = (start_x + pixels_copied) % self.get_width();
        let new_y = start_y + (pixels_copied / self.get_width());

        (new_x, new_y)
    }

    /// Returns the number of pixels copied
    #[cfg(not(feature = "time-tracking"))] // We don't have any timestamp knowledge in this case
    fn set_multi_from_start_index(&self, starting_index: usize, pixels: &[u8]) -> usize;

    /// (Only the pixel colors) as raw bytes.
    ///
    /// As the pixel memory doesn't necessarily need to be aligned (think of using shared memory for
    /// that), we can only return it as a list of bytes, not a list of pixels.
    //
    // We don't have a continues memory representation of the pixels colors.
    #[cfg(not(feature = "time-tracking"))]
    fn pixel_color_bytes(&self) -> &[u8];

    /// Calculates the timestamp to be used for pixels based on the current time
    #[cfg(feature = "time-tracking")]
    fn current_ts(&self) -> u64;
}
