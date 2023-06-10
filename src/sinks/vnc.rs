use crate::framebuffer::FrameBuffer;
use crate::statistics::{StatisticsEvent, StatisticsInformationEvent};
use core::slice;
use number_prefix::NumberPrefix;
use rusttype::{point, Font, Scale};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use vncserver::{
    rfb_framebuffer_malloc, rfb_get_screen, rfb_init_server, rfb_mark_rect_as_modified,
    rfb_run_event_loop, RfbScreenInfoPtr,
};

const STATS_HEIGHT: usize = 35;

pub struct VncServer<'a> {
    fb: Arc<FrameBuffer>,
    screen: RfbScreenInfoPtr,
    target_fps: u32,

    statistics_tx: Sender<StatisticsEvent>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,

    text: &'a str,
    font: Font<'a>,
}

impl<'a> VncServer<'a> {
    pub fn new(
        fb: Arc<FrameBuffer>,
        port: u32,
        target_fps: u32,
        statistics_tx: Sender<StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        text: &'a str,
        font: &'a str,
    ) -> Self {
        let screen = rfb_get_screen(fb.get_width() as i32, fb.get_height() as i32, 8, 3, 4);
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

        rfb_framebuffer_malloc(screen, (fb.get_size() * 4/* bytes per pixel */) as u64);
        rfb_init_server(screen);
        rfb_run_event_loop(screen, 1, 1);

        let font = match font {
            // We ship our own copy of Arial.ttf, so that users don't need to download and provide it
            "Arial.ttf" => {
                let font_bytes = include_bytes!("../../Arial.ttf");
                Font::try_from_bytes(font_bytes)
                    .unwrap_or_else(|| panic!("Failed to construct Font from Arial.ttf"))
            }
            _ => {
                let font_bytes = std::fs::read(font)
                    .unwrap_or_else(|err| panic!("Failed to read font file {font}: {err}"));
                Font::try_from_vec(font_bytes)
                    .unwrap_or_else(|| panic!("Failed to construct Font from font file {font}"))
            }
        };

        VncServer {
            fb,
            screen,
            target_fps,
            statistics_tx,
            statistics_information_rx,
            text,
            font,
        }
    }

    pub fn run(&mut self) {
        let target_loop_duration = Duration::from_micros(1_000_000 / self.target_fps as u64);

        let fb = &self.fb;
        let vnc_fb_slice: &mut [u32] = unsafe {
            slice::from_raw_parts_mut((*self.screen).frameBuffer as *mut u32, fb.get_size())
        };
        let fb_slice = unsafe { &*fb.get_buffer() };
        // A line less because the (height - STATS_SURFACE_HEIGHT) belongs to the stats and gets refreshed by them
        let height_up_to_stats_text = self.fb.get_height() - STATS_HEIGHT - 1;
        let fb_size_up_to_stats_text = fb.get_width() * height_up_to_stats_text;

        loop {
            let start = std::time::Instant::now();
            vnc_fb_slice[0..fb_size_up_to_stats_text]
                .copy_from_slice(&fb_slice[0..fb_size_up_to_stats_text]);

            // Only refresh the drawing surface, not the stats surface
            rfb_mark_rect_as_modified(
                self.screen,
                0,
                0,
                self.fb.get_width() as i32,
                height_up_to_stats_text as i32,
            );
            self.statistics_tx
                .blocking_send(StatisticsEvent::FrameRendered)
                .unwrap();

            if !self.statistics_information_rx.is_empty() {
                let statistics_information_event =
                    self.statistics_information_rx.try_recv().unwrap();
                self.display_stats(statistics_information_event);
            }

            std::thread::sleep(target_loop_duration.saturating_sub(start.elapsed()));
        }
    }

    fn display_stats(&mut self, stats: StatisticsInformationEvent) {
        self.draw_rect(
            0,
            self.fb.get_height() - STATS_HEIGHT,
            self.fb.get_width(),
            self.fb.get_height(),
            0,
        );
        self.draw_text(
            20,
            self.fb.get_height() - STATS_HEIGHT + 2,
            27_f32,
            0x00ff_ffff,
            format!(
                "{}. {} Bit/s ({}B total) by {} connections from {} IPs ({} legacy)",
                self.text,
                format_per_s(stats.bytes_per_s as f64 * 8.0),
                format(stats.bytes as f64),
                stats.connections,
                stats.ips,
                stats.legacy_ips,
            )
            .as_str(),
        );

        // Only refresh the stats surface, not the drawing surface
        rfb_mark_rect_as_modified(
            self.screen,
            0,
            (self.fb.get_height() - STATS_HEIGHT) as i32,
            self.fb.get_width() as i32,
            self.fb.get_height() as i32,
        );
    }

    fn draw_text(&mut self, x: usize, y: usize, scale: f32, text_rgba: u32, text: &str) {
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

    fn draw_rect(&mut self, start_x: usize, start_y: usize, end_x: usize, end_y: usize, rgba: u32) {
        for x in start_x..end_x {
            for y in start_y..end_y {
                self.set_pixel_checked(x, y, rgba);
            }
        }
    }

    /// Check for bounds. If out of bound do nothing.
    fn set_pixel_checked(&mut self, x: usize, y: usize, rgba: u32) {
        if x < self.fb.get_width() && y < self.fb.get_height() {
            unsafe {
                let addr = (*self.screen).frameBuffer as *mut u32;
                let slice: &mut [u32] = slice::from_raw_parts_mut(addr, self.fb.get_size());
                slice[x + self.fb.get_width() * y] = rgba;
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
