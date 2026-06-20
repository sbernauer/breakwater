pub mod shared_memory;
pub mod simple;
pub mod time_tracking;

/// Non-time-tracking framebuffer use an [`u32`] to store the color value
pub const FB_BYTES_PER_PIXEL: usize = std::mem::size_of::<u32>();

pub trait FrameBuffer {
    type Timestamp: Copy = ();

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

    /// Set the pixel color and (potentially) track the timestamp it was set.
    ///
    /// Note that most framebuffers don't care about the timestamp.
    ///
    /// The timestamp is an arbitrary type, it's actual value is defined by the framebuffer - e.g.
    /// microseconds since startup or anything else.
    ///
    /// Will do nothing if the coordinates are out of bounds.
    ///
    /// Performance consideration: We use `#[inline(always)]` and dead-store elimination should
    /// remove any overhead of this additional, unused, function argument.
    fn set(&self, x: usize, y: usize, rgba: u32, ts: Self::Timestamp);

    /// Calculates the timestamp to be used for pixels based on the current time.
    ///
    /// Note that most framebuffers don't care about the timestamp and will just return `()`.
    ///
    /// Performance consideration: It's ok to go and fetch the system time, which might take a
    /// little(!) bit. Parsers are expected to not call this per parsed pixel, but fetch it and
    /// cache it for a sane amount of time, e.g. per parse invocation.
    fn current_ts(&self) -> Self::Timestamp;
}

pub trait MultiPixelSet: FrameBuffer {
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
}

pub trait PixelColorBytes {
    /// (Only the pixel colors) as raw bytes.
    ///
    /// As the pixel memory doesn't necessarily need to be aligned (think of using shared memory for
    /// that), we can only return it as a list of bytes, not a list of pixels.
    fn pixel_color_bytes(&self) -> &[u8];
}
