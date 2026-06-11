use std::time::{SystemTime, UNIX_EPOCH};

use tracing::debug;

use crate::FrameBuffer;

pub struct TimeTrackingFrameBuffer {
    width: usize,
    height: usize,
    buffer: Vec<TimeTrackingPixel>,
    base_ns_since_unix_epoch: u64,
}

/// Number of low bits we drop from `ns_since_base` before storing it. We only have 3 bytes
/// (24 bits) of storage for it, so dropping 5 bits (i.e. dividing by 32) lets us cover
/// `0xff_ffff * 32` ns ≈ 536 ms before we have to clamp. That's plenty given the base
/// timestamp is re-based frequently, and 32 ns of resolution is way more than we need.
const NS_SHIFT: u32 = 5;

/// Largest value we can store in the 3 `ns` bytes.
const NS_MAX: u32 = 0x00ff_ffff;

/// A single pixel, laid out as exactly 6 bytes (`align = 1`):
///
/// ```text
/// byte:  0     1     2     3     4     5
///       [ r ] [ g ] [ b ] [ns0] [ns1] [ns2]
/// ```
///
/// The `ns` bytes hold `ns_since_base >> NS_SHIFT` (see [`NS_SHIFT`]), clamped to [`NS_MAX`].
///
/// We deliberately *don't* expose the fields as `u32`s (which would force `align = 4` and 8
/// bytes per pixel). The trade-off of going to 6 bytes: 6 does not divide 64, so ~10% of
/// pixels straddle a cache-line boundary. The *write* path (random, hot) therefore uses
/// byte/`[u8; 3]` stores rather than wide `u32`/`u16` stores — a wide store that crosses the
/// boundary triggers the x86 split-store penalty on every such pixel, which measured slower
/// (~65G vs ~76G).
///
/// Both reads and writes only ever touch the bytes of their own pixel, so no pixel can tear a
/// neighbour and no access runs past the end of the buffer — no padding is required.
///
/// Note: 6-byte cells don't quite match the original 8-byte *aligned* layout (~90G vs ~86G
/// here) on a write-bound load — that one is fast precisely because every write stays within
/// one cache line. The ~4% is the cost of shrinking the per-pixel sync footprint by 25%.
#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct TimeTrackingPixel {
    pub rgb: [u8; 3],
    /// `ns_since_base >> NS_SHIFT`, little-endian, clamped to `NS_MAX`.
    pub coarse_ns_since_base: [u8; 3],
}

impl TimeTrackingFrameBuffer {
    pub fn new(width: usize, height: usize, base_ns_since_unix_epoch: u64) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, TimeTrackingPixel::default);

        debug!(
            size = buffer.len(),
            bytes = buffer.len() * size_of::<TimeTrackingPixel>(),
            "Allocated time tracking framebuffer"
        );
        Self {
            width,
            height,
            buffer,
            base_ns_since_unix_epoch,
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
        // Assemble the RGB bytes; the 4th byte (alpha) is always zero. This is the sequential
        // scanout path, which is bandwidth-bound, so the byte assembly is hidden under the
        // memory stream and reads no wider than the 3 RGB bytes.
        let rgb = unsafe { self.buffer.get_unchecked(pixel_index).rgb };
        u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 0])
    }

    fn set(&self, _: usize, _: usize, _: u32) {
        panic!(
            "The time tracking framebuffer requires you to use the set_with_ns_since_unix_epoch function!"
        );
    }

    fn set_with_ns_since_unix_epoch(
        &self,
        x: usize,
        y: usize,
        rgba: u32,
        ns_since_unix_epoch: u64,
    ) {
        if x < self.width && y < self.height {
            let pixel_index = self.pixel_index(x, y);

            // Drop the low NS_SHIFT bits and clamp into our 3 ns bytes. If the base timestamp is
            // more than ~536 ms in the past (should not happen, it gets re-based regularly) we
            // saturate to NS_MAX.
            let raw_coarse_ns = (ns_since_unix_epoch - self.base_ns_since_unix_epoch) >> NS_SHIFT;

            // Commented out while benchmarking on a debug build (the warning would otherwise fire
            // on the hot path). Re-enable to check whether the ~536 ms window is ever exceeded.
            // #[cfg(debug_assertions)]
            // if raw_coarse_ns > u64::from(NS_MAX) {
            //     tracing::warn!(
            //         base_ns_since_unix_epoch = self.base_ns_since_unix_epoch,
            //         ns_since_unix_epoch,
            //         raw_coarse_ns,
            //         ns_max = NS_MAX,
            //         "A pixel was set more than ~536ms after the last base timestamp; this should \
            //          not happen. Clamping it to ~536ms"
            //     );
            // }

            let coarse_ns_since_base = u32::try_from(raw_coarse_ns)
                .unwrap_or(u32::MAX)
                .min(NS_MAX)
                .to_le_bytes();

            // Write via the byte-array fields, *not* via wide unaligned u32/u16 stores. A 6-byte
            // stride means ~10% of pixels straddle a 64-byte cache line, and a single wide store
            // that crosses the boundary hits the x86 split-store penalty on every such write
            // (random writes are the hot path here). Byte/u16-sized stores rarely split, which is
            // why this is meaningfully faster than packing into one u32 + one u16.
            unsafe {
                let ptr: *mut TimeTrackingPixel = self.buffer.as_ptr().add(pixel_index).cast_mut();
                (*ptr).rgb = [rgba as u8, (rgba >> 8) as u8, (rgba >> 16) as u8];
                (*ptr).coarse_ns_since_base = [
                    coarse_ns_since_base[0],
                    coarse_ns_since_base[1],
                    coarse_ns_since_base[2],
                ];
            }
        }
    }

    fn set_multi_from_start_index(&self, _: usize, _: &[u8]) -> usize {
        panic!("The time tracking framebuffer does not implement set_multi_from_start_index");
    }

    fn as_bytes(&self) -> &[u8] {
        todo!("We might actually can and want to implement TimeTrackingFrameBuffer::as_bytes")
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
