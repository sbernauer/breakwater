use core::slice;
use std::sync::atomic::Ordering::Relaxed;
use rusttype::{point, Font, Scale};
use std::thread;
use std::time::Duration;
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
    pub fn new(fb: &'a FrameBuffer, fps: u32, text: &'a str, statistics: &'a Statistics) -> Self {
        let screen = rfb_get_screen(fb.width as i32, fb.height as i32, 8, 3, 4);

        rfb_framebuffer_malloc(screen, (fb.width * fb.height * 4 /* bytes per pixel */) as u64);
        rfb_init_server(screen);
        rfb_run_event_loop(screen, 1, 1);

        let font_data = include_bytes!("../Arial.ttf");
        let font = Font::try_from_bytes(font_data as &[u8]).expect("Error constructing Font");

        VncServer{
            fb,
            screen,
            fps,
            font,
            text,
            statistics,
        }
    }

    pub fn run(&self) {
        loop {
            for x in 0..self.fb.width {
                for y in 0..self.fb.height {
                    // We don't use this as the wrapper method only exists for 16 bit, not for 32 bit :/
                    // rfb_framebuffer_set_rgb16(vnc_server, x as i32, y as i32, fb.get(x, y));
                    self.set_pixel(x, y, self.fb.get(x, y));
                }
            }
            self.draw_text(20_f32, 10_f32, 32_f32, self.text);
            self.draw_text(20_f32, 50_f32, 32_f32,
               format!("{} connections", self.statistics.connections.load(Relaxed)).as_str());
            rfb_mark_rect_as_modified(self.screen, 0, 0, self.fb.width as i32, self.fb.height as i32);

            thread::sleep(Duration::from_millis(1000 / self.fps as u64)); // TODO Measure loop time and subtract it
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

    fn draw_text(&self, x: f32, y: f32, scale: f32, text: &str) {
        let scale = Scale::uniform(scale);

        let v_metrics = self.font.v_metrics(scale);

        let glyphs: Vec<_> = self.font
            .layout(text, scale, point(x, y + v_metrics.ascent))
            .collect();

        for glyph in glyphs {
            if let Some(bounding_box) = glyph.pixel_bounding_box() {
                glyph.draw(|x, y, v| {
                    if v > 0.5 {
                        self.set_pixel(
                            x as usize + bounding_box.min.x as usize,
                            y as usize + bounding_box.min.y as usize,
                            0x0000_ff00
                        )
                    }
                });
            }
        }
    }
}
