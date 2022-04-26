use core::slice;
use std::fs;
use std::sync::atomic::Ordering::{AcqRel, Acquire};
use std::thread;
use std::time::{Duration, Instant};

use number_prefix::NumberPrefix;
use rusttype::{point, Font, Scale};
use vncserver::*;

use crate::framebuffer::FrameBuffer;
use crate::Statistics;

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
            fs::read(font).unwrap_or_else(|_| panic!("Cannot read font file {}", font));
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
                for y in 0..self.fb.height {
                    // We don't use this as the wrapper method only exists for 16 bit, not for 32 bit :/
                    // rfb_framebuffer_set_rgb16(vnc_server, x as i32, y as i32, fb.get(x, y));
                    self.set_pixel(x, y, self.fb.get(x, y));
                }
            }
            self.draw_rect(
                25,
                self.fb.height - 100,
                self.fb.width - 25,
                self.fb.height - 25,
                0x0000_0000,
            );
            self.draw_text(
                30,
                self.fb.height - 100,
                32_f32,
                0x00ff_ffff,
                format!(
                    "{}. {} connections by {} IPs ({} legacy)",
                    self.text,
                    self.statistics.current_connections.load(Acquire),
                    self.statistics.current_ips.load(Acquire),
                    self.statistics.current_legacy_ips.load(Acquire)
                )
                .as_str(),
            );
            self.draw_text(
                30,
                self.fb.height - 70,
                32_f32,
                0x00ff_ffff,
                format!(
                    "{} Bit/s ({}B total). {} Pixel/s ({} Pixels total)",
                    format_per_s(self.statistics.bytes_per_s.load(Acquire) as f64 * 8.0),
                    format(self.statistics.current_bytes.load(Acquire) as f64),
                    format_per_s(self.statistics.pixels_per_s.load(Acquire) as f64),
                    format(self.statistics.current_pixels.load(Acquire) as f64),
                )
                .as_str(),
            );
            rfb_mark_rect_as_modified(
                self.screen,
                0,
                0,
                self.fb.width as i32,
                self.fb.height as i32,
            );

            self.statistics.frame.fetch_add(1, AcqRel);

            let duration_ms = start.elapsed().as_millis();
            if duration_ms < desired_loop_time_ms {
                thread::sleep(Duration::from_millis(
                    (desired_loop_time_ms - duration_ms) as u64,
                ));
            }
        }
    }

    // We don't check for bounds as the only input is from this struct
    fn set_pixel(&self, x: usize, y: usize, rgba: u32) {
        unsafe {
            let addr = (*self.screen).frameBuffer as *mut u32;
            let slice: &mut [u32] = slice::from_raw_parts_mut(addr, self.fb.width * self.fb.height);
            slice[x + self.fb.width * y] = rgba;
        }
    }

    fn draw_text(&self, x: usize, y: usize, scale: f32, rgba: u32, text: &str) {
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
                        // TODO Check for bounds
                        self.set_pixel(
                            x as usize + bounding_box.min.x as usize,
                            y as usize + bounding_box.min.y as usize,
                            rgba,
                        )
                    }
                });
            }
        }
    }

    fn draw_rect(&self, start_x: usize, start_y: usize, end_x: usize, end_y: usize, rgba: u32) {
        for x in start_x..end_x {
            for y in start_y..end_y {
                self.set_pixel(x, y, rgba);
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
