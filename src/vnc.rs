use core::slice;
use std::fs;
use std::sync::atomic::Ordering::{AcqRel, Acquire, SeqCst};
use std::thread;
use std::time::{Duration, Instant};

use number_prefix::NumberPrefix;
use rusttype::{point, Font, Scale};
use vncserver::*;

use crate::framebuffer::FrameBuffer;
use crate::Statistics;

const STATS_HEIGHT: usize = 35;

pub struct VncServer<'a> {
    fb: &'a FrameBuffer,
    screen: RfbScreenInfoPtr,
    fps: u32,
    font: Font<'a>,
    text: &'a str,
    statistics: &'a Statistics,
}

impl<'a> VncServer<'a> {
    pub fn new(
        fb: &'a FrameBuffer,
        port: u32,
        fps: u32,
        text: &'a str,
        statistics: &'a Statistics,
        font: &'a str,
    ) -> Self {
        let screen = rfb_get_screen(fb.width as i32, fb.height as i32, 8, 3, 4);
        unsafe {
            // We need to set bitsPerPixel and depth to the correct values,
            // otherwise some VNC clients (like gstreamer) won't work
            (*screen).bitsPerPixel = 32;
            (*screen).depth = 24;
            (*screen).serverFormat.depth = 24;
        }
        unsafe {
            (*screen).port = port as i32;
            (*screen).ipv6port = port as i32;
        }

        rfb_framebuffer_malloc(
            screen,
            (fb.width * fb.height * 4/* bytes per pixel */) as u64,
        );
        rfb_init_server(screen);
        rfb_run_event_loop(screen, 1, 1);

        let font_bytes =
            fs::read(font).unwrap_or_else(|err| panic!("Cannot read font file {}: {}", font, err));
        let font = Font::try_from_vec(font_bytes).expect("Error constructing Font");

        VncServer {
            fb,
            screen,
            fps,
            font,
            text,
            statistics,
        }
    }

    pub fn run(&self) {
        let desired_loop_time_ms = (1_000 / self.fps) as u128;
        loop {
            let start = Instant::now();

            for x in 0..self.fb.width {
                for y in 0..(self.fb.height - STATS_HEIGHT) {
                    self.set_pixel_unchecked(x, y, self.fb.get(x, y));
                }
            }
            // Only refresh the drawing surface, not the stats surface
            rfb_mark_rect_as_modified(
                self.screen,
                0,
                0,
                self.fb.width as i32,
                // A line less because the (height - STATS_SURFACE_HEIGHT) belongs to the stats and get refreshed by them
                (self.fb.height - STATS_HEIGHT - 1) as i32,
            );

            // If the statistics thread has produced new stats it flags this for us so that we can re-draw the stats *once*.
            // If we draw them every frame we get a flickering and produce unnecessary VNC updates.
            if !self.statistics.stats_on_screen_are_up_to_date.load(SeqCst) {
                self.statistics
                    .stats_on_screen_are_up_to_date
                    .store(true, SeqCst);
                self.update_stats();
            }

            self.statistics.frame.fetch_add(1, AcqRel);

            let duration_ms = start.elapsed().as_millis();
            if duration_ms < desired_loop_time_ms {
                thread::sleep(Duration::from_millis(
                    (desired_loop_time_ms - duration_ms) as u64,
                ));
            }
        }
    }

    /// Don't check for bounds as input is assumed to be safe for performance reasons
    fn set_pixel_unchecked(&self, x: usize, y: usize, rgba: u32) {
        unsafe {
            let addr = (*self.screen).frameBuffer as *mut u32;
            let slice: &mut [u32] = slice::from_raw_parts_mut(addr, self.fb.width * self.fb.height);
            slice[x + self.fb.width * y] = rgba;
        }
    }

    /// Check for bounds. If out of bound do nothing.
    fn set_pixel_checked(&self, x: usize, y: usize, rgba: u32) {
        if x < self.fb.width && y < self.fb.height {
            unsafe {
                let addr = (*self.screen).frameBuffer as *mut u32;
                let slice: &mut [u32] =
                    slice::from_raw_parts_mut(addr, self.fb.width * self.fb.height);
                slice[x + self.fb.width * y] = rgba;
            }
        }
    }

    pub fn update_stats(&self) {
        self.draw_rect(
            0,
            self.fb.height - STATS_HEIGHT,
            self.fb.width,
            self.fb.height,
            0x0000_0000,
        );
        self.draw_text(
            20,
            self.fb.height - STATS_HEIGHT + 2,
            27_f32,
            0x00ff_ffff,
            format!(
                "{}. {} Bit/s ({}B total) by {} connections from {} IPs ({} legacy)",
                self.text,
                format_per_s(self.statistics.bytes_per_s.load(Acquire) as f64 * 8.0),
                format(self.statistics.current_bytes.load(Acquire) as f64),
                self.statistics.current_connections.load(Acquire),
                self.statistics.current_ips.load(Acquire),
                self.statistics.current_legacy_ips.load(Acquire),
            )
            .as_str(),
        );

        // Only refresh the stats surface, not the drawing surface
        rfb_mark_rect_as_modified(
            self.screen,
            0,
            (self.fb.height - STATS_HEIGHT) as i32,
            self.fb.width as i32,
            self.fb.height as i32,
        );
    }

    fn draw_text(&self, x: usize, y: usize, scale: f32, text_rgba: u32, text: &str) {
        let scale = Scale::uniform(scale);

        let v_metrics = self.font.v_metrics(scale);

        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(x as f32, y as f32 + v_metrics.ascent))
            .collect();

        for glyph in glyphs {
            if let Some(bounding_box) = glyph.pixel_bounding_box() {
                glyph.draw(|x, y, v| {
                    if v > 0.5 {
                        self.set_pixel_checked(
                            x as usize + bounding_box.min.x as usize,
                            y as usize + bounding_box.min.y as usize,
                            text_rgba,
                        )
                    }
                });
            }
        }
    }

    fn draw_rect(&self, start_x: usize, start_y: usize, end_x: usize, end_y: usize, rgba: u32) {
        for x in start_x..end_x {
            for y in start_y..end_y {
                self.set_pixel_checked(x, y, rgba);
            }
        }
    }
}

fn format_per_s(value: f64) -> String {
    match NumberPrefix::decimal(value) {
        NumberPrefix::Prefixed(prefix, n) => format!("{n:.1}{prefix}"),
        NumberPrefix::Standalone(n) => format!("{n}"),
    }
}

fn format(value: f64) -> String {
    match NumberPrefix::decimal(value) {
        NumberPrefix::Prefixed(prefix, n) => format!("{n:.1}{prefix}"),
        NumberPrefix::Standalone(n) => format!("{n}"),
    }
}
