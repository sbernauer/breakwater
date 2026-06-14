use std::time::{SystemTime, UNIX_EPOCH};

use crate::FrameBuffer;

/// Number of low bits holding the RGB color; the rest hold the timestamp.
const RGB_BITS: u32 = 24;
const RGB_MASK: u64 = (1 << RGB_BITS) - 1;

/// Largest timestamp we can store in the remaining `64 - RGB_BITS = 40` bits. In microseconds
/// that's ≈ 12.7 days of collector uptime before it saturates — plenty, and a collector restart
/// resets the epoch anyway.
const TIMESTAMP_MAX: u64 = (1 << (u64::BITS - RGB_BITS)) - 1;

/// A single pixel packed into one `u64`: the low [`RGB_BITS`] bits are the RGB color, the high bits
/// are an opaque timestamp. Packing both into a `u64` means writes are a single aligned 64-bit
/// store (the hot path).
///
/// The timestamp is deliberately opaque here — its *meaning* (microseconds since the collector
/// epoch) is assigned solely by [`TimeTrackingFrameBuffer::pixel_timestamp`]. To everyone else it's
/// just a comparable number: larger = more recently written, and `0` (the default) = never written,
/// which compares as the oldest possible, so it never wins a merge.
#[derive(Debug, Default, Clone, Copy)]
#[repr(transparent)]
pub struct TimeTrackingPixel(u64);

impl TimeTrackingPixel {
    pub fn new(rgb: u32, timestamp: u64) -> Self {
        Self((timestamp << RGB_BITS) | (u64::from(rgb) & RGB_MASK))
    }

    /// The 24-bit RGB color (the implicit alpha byte is always zero).
    pub fn rgb(self) -> u32 {
        (self.0 & RGB_MASK) as u32
    }

    /// The opaque write timestamp (see the type docs); larger is newer, `0` is never written.
    pub fn timestamp(self) -> u64 {
        self.0 >> RGB_BITS
    }
}

/// Views a slice of pixels as their raw bytes (8 per pixel), e.g. to read a frame straight off the
/// wire into a `Vec<TimeTrackingPixel>`. Same layout as [`FrameBuffer::as_bytes`].
pub fn pixels_as_bytes_mut(pixels: &mut [TimeTrackingPixel]) -> &mut [u8] {
    let len = size_of_val(pixels);
    let ptr = pixels.as_mut_ptr().cast::<u8>();
    // SAFETY: `TimeTrackingPixel` is `repr(transparent)` over a `u64` (all bit patterns valid), so
    // its bytes are a valid `[u8]` of the same lifetime and exclusive borrow.
    unsafe { std::slice::from_raw_parts_mut(ptr, len) }
}

pub struct TimeTrackingFrameBuffer {
    width: usize,
    height: usize,
    buffer: Vec<TimeTrackingPixel>,
    /// Per-pixel timestamps are microseconds since this epoch — the collector's startup time, in ns
    /// since the UNIX epoch. Fixed for the framebuffer's life, so there is no re-basing.
    epoch_ns_since_unix_epoch: u64,
}

impl TimeTrackingFrameBuffer {
    pub fn new(width: usize, height: usize, epoch_ns_since_unix_epoch: u64) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, TimeTrackingPixel::default);

        Self {
            width,
            height,
            buffer,
            epoch_ns_since_unix_epoch,
        }
    }

    #[inline(always)]
    fn pixel_index(&self, x: usize, y: usize) -> usize {
        x + y * self.width
    }
}

impl FrameBuffer for TimeTrackingFrameBuffer {
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
        let pixel_index = self.pixel_index(x, y);
        unsafe { self.buffer.get_unchecked(pixel_index).rgb() }
    }

    fn set(&self, _: usize, _: usize, _: u32) {
        panic!(
            "The time tracking framebuffer requires you to use the set_with_pixel_timestamp function!"
        );
    }

    fn pixel_timestamp(&self, ns_since_unix_epoch: u64) -> u64 {
        // The per-parse-call timestamp, computed once. `saturating_sub` guards against a worker
        // whose clock trails the collector's epoch slightly; `.min` clamps the ~12.7 day range.
        (ns_since_unix_epoch.saturating_sub(self.epoch_ns_since_unix_epoch) / 1_000)
            .min(TIMESTAMP_MAX)
    }

    fn set_with_pixel_timestamp(&self, x: usize, y: usize, rgba: u32, pixel_timestamp: u64) {
        if x < self.width && y < self.height {
            let pixel_index = self.pixel_index(x, y);
            // A single aligned 64-bit store (the whole point of packing the pixel into a `u64`).
            unsafe {
                let ptr: *mut TimeTrackingPixel = self.buffer.as_ptr().add(pixel_index).cast_mut();
                *ptr = TimeTrackingPixel::new(rgba, pixel_timestamp);
            }
        }
    }

    fn set_multi_from_start_index(&self, _: usize, _: &[u8]) -> usize {
        panic!("The time tracking framebuffer does not implement set_multi_from_start_index");
    }

    fn as_bytes(&self) -> &[u8] {
        // The buffer is a contiguous `Vec<TimeTrackingPixel>`, so its raw bytes are exactly the
        // 8-bytes-per-pixel wire layout. Like the other framebuffers, this reads memory that writers
        // may be mutating concurrently — fine for a lossy, best-effort sync.
        let len = self.buffer.len() * size_of::<TimeTrackingPixel>();
        let ptr = self.buffer.as_ptr().cast::<u8>();
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

/// Don't call too often, there is a cost involved!
pub fn get_current_ns_since_unix_epoch() -> u64 {
    let ns_since_unix_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // TODO: If this turns out to be correct, convert to `unwrap_unchecked`
        .expect("your system clock must be after UNIX EPOCH (so greater than 0)")
        .as_nanos();

    // u64::MAX allows us 18446744073709551615 ns since UNIX_EPOCH, which is
    // some time in the year 2554, well beyond any reasonable timestamp.
    u64::try_from(ns_since_unix_epoch).expect(
        "your system time is >= year 2554. I'm developing this in 2026, I'm very likely dead now. And did no one write a better server to use in all that years?",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_packs_rgb_and_timestamp() {
        let pixel = TimeTrackingPixel::new(0x12_3456, 1_000_000);
        assert_eq!(pixel.rgb(), 0x12_3456);
        assert_eq!(pixel.timestamp(), 1_000_000);
    }

    #[test]
    fn pixel_timestamp_is_micros_since_epoch_clamped() {
        let fb = TimeTrackingFrameBuffer::new(1, 1, 1_000_000);

        // 1_000_000 ns after the epoch -> 1000 µs.
        assert_eq!(fb.pixel_timestamp(2_000_000), 1_000);
        // A timestamp before the epoch saturates to 0 (clock skew guard).
        assert_eq!(fb.pixel_timestamp(0), 0);
        // Beyond the 40-bit range it clamps.
        assert_eq!(fb.pixel_timestamp(u64::MAX), TIMESTAMP_MAX);
    }

    #[test]
    fn set_and_read_back_pixel() {
        let fb = TimeTrackingFrameBuffer::new(4, 4, 0);
        fb.set_with_pixel_timestamp(1, 2, 0x00_ff00, 42);

        let pixel = fb.buffer[fb.pixel_index(1, 2)];
        assert_eq!(pixel.rgb(), 0x00_ff00);
        assert_eq!(pixel.timestamp(), 42);
        assert_eq!(fb.get(1, 2), Some(0x00_ff00));
    }
}
