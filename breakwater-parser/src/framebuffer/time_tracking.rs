use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use tracing::warn;

use crate::FrameBuffer;

pub struct TimeTrackingFrameBuffer {
    width: usize,
    height: usize,
    buffer: Vec<TimeTrackingPixel>,
    /// All per-pixel timestamps are stored relative to this base. A background task re-bases it
    /// (to the current time) at a fixed rate, which keeps `ns_since_base` small enough to fit in
    /// our 3 bytes. Read on the hot write path and written by the re-basing task, hence atomic.
    /// A `Relaxed` load compiles to a plain `mov` on x86_64, so it doesn't cost us anything.
    base_ns_since_unix_epoch: AtomicU64,
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
///        [ r ] [ g ] [ b ] [ns0] [ns1] [ns2]
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

/// Views a slice of pixels as their raw bytes (6 per pixel), e.g. to read a frame straight off the
/// wire into a `Vec<TimeTrackingPixel>`. Same layout as [`FrameBuffer::as_bytes`].
pub fn pixels_as_bytes_mut(pixels: &mut [TimeTrackingPixel]) -> &mut [u8] {
    let len = size_of_val(pixels);
    let ptr = pixels.as_mut_ptr().cast::<u8>();
    // SAFETY: `TimeTrackingPixel` is `repr(C)`, `align = 1`, all-bytes-valid (just `[u8; 3]`s), so
    // its `len * 6` bytes are a valid `[u8]` of the same lifetime and exclusive borrow.
    unsafe { std::slice::from_raw_parts_mut(ptr, len) }
}

impl TimeTrackingPixel {
    /// The stored `coarse_ns_since_base` as a `u32` (the implicit high byte is always zero).
    fn coarse_ns_since_base(self) -> u32 {
        u32::from_le_bytes([
            self.coarse_ns_since_base[0],
            self.coarse_ns_since_base[1],
            self.coarse_ns_since_base[2],
            0,
        ])
    }

    /// The absolute UNIX-epoch time this pixel was last written, given the framebuffer's
    /// `base_ns_since_unix_epoch`, reconstructed at `1 << NS_SHIFT` ns resolution.
    ///
    /// Returns `None` when the pixel carries no recent-write information (`coarse_ns == 0`): the
    /// framebuffer re-bases every frame, so a pixel not written within the last window saturates to
    /// 0 and is indistinguishable from "never written". Callers merging framebuffers should treat
    /// `None` as "no update from this pixel" so stale/blank pixels don't clobber fresher content.
    pub fn written_ns_since_unix_epoch(self, base_ns_since_unix_epoch: u64) -> Option<u64> {
        let coarse = self.coarse_ns_since_base();
        (coarse != 0).then(|| base_ns_since_unix_epoch + (u64::from(coarse) << NS_SHIFT))
    }
}

impl TimeTrackingFrameBuffer {
    pub fn new(width: usize, height: usize, base_ns_since_unix_epoch: u64) -> Self {
        let mut buffer = Vec::with_capacity(width * height);
        buffer.resize_with(width * height, TimeTrackingPixel::default);

        Self {
            width,
            height,
            buffer,
            base_ns_since_unix_epoch: AtomicU64::new(base_ns_since_unix_epoch),
        }
    }

    #[inline(always)]
    fn pixel_index(&self, x: usize, y: usize) -> usize {
        x + y * self.width
    }

    /// The current base all per-pixel timestamps are relative to. Capture this *before* syncing a
    /// frame, so the consumer can interpret that frame's `coarse_ns_since_base` values.
    pub fn base_ns_since_unix_epoch(&self) -> u64 {
        self.base_ns_since_unix_epoch.load(Ordering::Relaxed)
    }

    /// Re-base all future per-pixel timestamps to `base_ns_since_unix_epoch`. Call this
    /// regularly (faster than the ~536 ms window) so `ns_since_base` keeps fitting in 3 bytes.
    pub fn set_base_ns_since_unix_epoch(&self, base_ns_since_unix_epoch: u64) {
        self.base_ns_since_unix_epoch
            .store(base_ns_since_unix_epoch, Ordering::Relaxed);
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
            "The time tracking framebuffer requires you to use the set_with_coarse_ns_since_base function!"
        );
    }

    fn coarse_ns_since_base(&self, ns_since_unix_epoch: u64) -> u32 {
        // The single point that reads the (atomic) base, called once per parse call. Drop the
        // low NS_SHIFT bits and clamp into our 3 ns bytes; if the base is more than ~536 ms in
        // the past (shouldn't happen, it gets re-based regularly) we saturate to NS_MAX.
        //
        // `saturating_sub`, not `-`: the parser captures `ns_since_unix_epoch` once at the start
        // of a parse call, but the background task may re-base `base` to a newer time in between.
        // In that case the pixels were effectively written at the new base, so a difference of 0
        // is exactly right.
        let base_ns_since_unix_epoch = self.base_ns_since_unix_epoch.load(Ordering::Relaxed);
        let raw_coarse_ns =
            ns_since_unix_epoch.saturating_sub(base_ns_since_unix_epoch) >> NS_SHIFT;

        if raw_coarse_ns > u64::from(NS_MAX) {
            warn!(
                base_ns_since_unix_epoch,
                ns_since_unix_epoch,
                raw_coarse_ns,
                ns_max = NS_MAX,
                "Pixels were written more than ~536ms after the last base timestamp; clamping to \
                 ~536ms. Is the framebuffer re-basing task still running?"
            );
        }

        u32::try_from(raw_coarse_ns).unwrap_or(u32::MAX).min(NS_MAX)
    }

    fn set_with_coarse_ns_since_base(
        &self,
        x: usize,
        y: usize,
        rgba: u32,
        coarse_ns_since_base: u32,
    ) {
        if x < self.width && y < self.height {
            let pixel_index = self.pixel_index(x, y);
            let coarse_ns_since_base = coarse_ns_since_base.to_le_bytes();

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
        // The buffer is a contiguous `Vec<TimeTrackingPixel>` of `align = 1` cells, so the raw
        // bytes are exactly the 6-bytes-per-pixel wire layout. Like the other framebuffers, this
        // reads memory that writers may be mutating concurrently — fine for a lossy, best-effort
        // sync (a torn pixel just gets corrected on the next frame).
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
