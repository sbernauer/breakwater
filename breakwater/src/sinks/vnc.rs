use std::{sync::Arc, time::Duration};

use breakwater_parser::FrameBuffer;
use core::slice;
use number_prefix::NumberPrefix;
use rusttype::{point, Font, Scale};
use snafu::{OptionExt, ResultExt, Snafu};
use tokio::sync::{
    broadcast,
    mpsc::{self, Sender},
    oneshot,
};
use vncserver::{
    rfb_framebuffer_malloc, rfb_get_screen, rfb_init_server, rfb_mark_rect_as_modified,
    rfb_run_event_loop, RfbScreenInfoPtr,
};

use crate::statistics::{StatisticsEvent, StatisticsInformationEvent};

const STATS_HEIGHT: usize = 35;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to read font from file {font_file}"))]
    ReadFontFile {
        source: std::io::Error,
        font_file: String,
    },

    #[snafu(display("Failed to construct font from font file {font_file}"))]
    ConstructFontFromFontFile { font_file: String },

    #[snafu(display("Failed to write to statistics channel"))]
    WriteToStatisticsChannel {
        source: mpsc::error::SendError<StatisticsEvent>,
    },

    #[snafu(display("Failed to read from statistics information channel"))]
    ReadFromStatisticsInformationChannel {
        source: broadcast::error::TryRecvError,
    },
}

// Sorry! Help needed :)
unsafe impl<'a, FB: FrameBuffer> Send for VncServer<'a, FB> {}

pub struct VncServer<'a, FB: FrameBuffer> {
    fb: Arc<FB>,
    screen: RfbScreenInfoPtr,
    target_fps: u32,

    statistics_tx: Sender<StatisticsEvent>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    terminate_signal_tx: oneshot::Receiver<()>,

    text: String,
    font: Font<'a>,
}

impl<'a, FB: FrameBuffer> VncServer<'a, FB> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fb: Arc<FB>,
        port: u16,
        target_fps: u32,
        statistics_tx: Sender<StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_tx: oneshot::Receiver<()>,
        text: String,
        font: String,
    ) -> Result<Self, Error> {
        let font = match font.as_str() {
            // We ship our own copy of Arial.ttf, so that users don't need to download and provide it
            "Arial.ttf" => {
                let font_bytes = include_bytes!("../../../Arial.ttf");
                Font::try_from_bytes(font_bytes).context(ConstructFontFromFontFileSnafu {
                    font_file: "Arial.ttf".to_string(),
                })?
            }
            _ => {
                let font_bytes = std::fs::read(&font).context(ReadFontFileSnafu {
                    font_file: font.to_string(),
                })?;

                Font::try_from_vec(font_bytes).context(ConstructFontFromFontFileSnafu {
                    font_file: font.to_string(),
                })?
            }
        };

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

        Ok(VncServer {
            fb,
            screen,
            target_fps,
            statistics_tx,
            statistics_information_rx,
            terminate_signal_tx,
            text,
            font,
        })
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let target_loop_duration = Duration::from_micros(1_000_000 / self.target_fps as u64);

        let vnc_fb_slice: &mut [u32] = unsafe {
            slice::from_raw_parts_mut((*self.screen).frameBuffer as *mut u32, self.fb.get_size())
        };

        // A line less because the (height - STATS_SURFACE_HEIGHT) belongs to the stats and gets refreshed by them
        let height_up_to_stats_text = self.fb.get_height() - STATS_HEIGHT - 1;
        let fb_size_up_to_stats_text = self.fb.get_width() * height_up_to_stats_text;

        loop {
            if self.terminate_signal_tx.try_recv().is_ok() {
                return Ok(());
            }

            let start = std::time::Instant::now();
            vnc_fb_slice[0..fb_size_up_to_stats_text]
                .copy_from_slice(&self.fb.as_pixels()[0..fb_size_up_to_stats_text]);

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
                .context(WriteToStatisticsChannelSnafu)?;

            if !self.statistics_information_rx.is_empty() {
                let statistics_information_event = self
                    .statistics_information_rx
                    .try_recv()
                    .context(ReadFromStatisticsInformationChannelSnafu)?;
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
