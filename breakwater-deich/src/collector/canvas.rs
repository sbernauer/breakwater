//! The long-term merged canvas.
//!
//! Every pixel keeps its full write timestamp (the high bits of [`TimeTrackingPixel`]), set by the
//! worker relative to the shared epoch. Merging is therefore exact last-write-wins per pixel — and,
//! crucially, *commutative*: merge order doesn't matter, so the collector can fold each worker's
//! latest framebuffer in whenever it arrives without any frame numbering or windowing. A
//! never-written pixel has timestamp `0` (the oldest possible), so a blank or restarted worker's
//! frame can never clobber live content, and the canvas keeps its contents across traffic gaps.

use std::sync::Arc;

use breakwater_parser::{FB_BYTES_PER_PIXEL, FrameBuffer, MultiPixelSet, TimeTrackingPixel};

/// A flat pixel vector — no width/height; merging is purely per-index.
pub struct Canvas {
    pixels: Vec<TimeTrackingPixel>,
    /// Reused scratch for [`Self::draw_to_framebuffer`]'s RGB byte layout, so the per-tick draw
    /// doesn't allocate a fresh multi-megabyte buffer at the frame rate.
    rgb_scratch: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        let pixel_count = width * height;
        Self {
            pixels: vec![TimeTrackingPixel::default(); pixel_count],
            rgb_scratch: Vec::with_capacity(pixel_count * FB_BYTES_PER_PIXEL),
        }
    }

    /// Folds `frame` in, keeping for each pixel whichever write has the larger timestamp.
    pub fn merge(&mut self, frame: &[TimeTrackingPixel]) {
        for (canvas_pixel, &frame_pixel) in self.pixels.iter_mut().zip(frame) {
            if frame_pixel.timestamp() > canvas_pixel.timestamp() {
                *canvas_pixel = frame_pixel;
            }
        }
    }

    pub fn draw_to_framebuffer<FB: FrameBuffer + MultiPixelSet>(&mut self, fb: &Arc<FB>) {
        self.rgb_scratch.clear();
        self.rgb_scratch
            .extend(self.pixels.iter().flat_map(|pixel| pixel.rgb().to_le_bytes()));
        fb.set_multi_from_start_index(0, &self.rgb_scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a frame from `(rgb, timestamp)` pairs.
    fn frame(pixels: &[(u32, u64)]) -> Vec<TimeTrackingPixel> {
        pixels
            .iter()
            .map(|&(rgb, timestamp)| TimeTrackingPixel::new(rgb, timestamp))
            .collect()
    }

    #[test]
    fn merges_latest_timestamp_per_pixel() {
        let mut canvas = Canvas::new(1, 2);

        // Pixel 0: A timestamp 1320, B timestamp 5032 -> B wins.
        // Pixel 1: A timestamp 4200, B timestamp 0 (never written) -> A stays.
        canvas.merge(&frame(&[(0xaa_0000, 1_320), (0xaa_0001, 4_200)]));
        canvas.merge(&frame(&[(0x00_00bb, 5_032), (0x00_00bc, 0)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x00_00bb);
        assert_eq!(canvas.pixels[0].timestamp(), 5_032);
        assert_eq!(canvas.pixels[1].rgb(), 0xaa_0001);
        assert_eq!(canvas.pixels[1].timestamp(), 4_200);
    }

    #[test]
    fn blank_frame_never_overwrites_live_content() {
        let mut canvas = Canvas::new(1, 1);
        canvas.merge(&frame(&[(0x12_3456, 50)]));

        // A never-written pixel has timestamp 0 (oldest possible), so it can't clobber live content
        // (the restarted-worker / blank-canvas case).
        canvas.merge(&frame(&[(0, 0)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x12_3456);
        assert_eq!(canvas.pixels[0].timestamp(), 50);
    }

    #[test]
    fn older_write_does_not_replace_newer() {
        let mut canvas = Canvas::new(1, 1);
        // Merge the newer write first, then an older one — order must not matter.
        canvas.merge(&frame(&[(0x00_00bb, 5_032)]));
        canvas.merge(&frame(&[(0xaa_0000, 1_320)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x00_00bb);
        assert_eq!(canvas.pixels[0].timestamp(), 5_032);
    }
}
