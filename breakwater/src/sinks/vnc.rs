use core::slice;
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use color_eyre::eyre::{self, Context, ContextCompat};
use number_prefix::NumberPrefix;
use rusttype::{Font, Scale, point};
use tokio::{
    sync::{broadcast, mpsc},
    time,
};
use vncserver::{
    RfbScreenInfoPtr, rfb_framebuffer_malloc, rfb_get_screen, rfb_init_server,
    rfb_mark_rect_as_modified, rfb_run_event_loop,
};

use crate::{
    cli_args::CliArgs,
    sinks::DisplaySink,
    statistics::{
        STATISTICS_INFO_RECV_ERR, STATISTICS_SEND_ERR, StatisticsEvent, StatisticsInformationEvent,
    },
};

const STATS_HEIGHT: usize = 35;

// Sorry! Help needed :)
unsafe impl<FB: FrameBuffer> Send for VncSink<'_, FB> {}

pub struct VncSink<'a, FB: FrameBuffer> {
    fb: Arc<FB>,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    terminate_signal_rx: broadcast::Receiver<()>,

    screen: RfbScreenInfoPtr,
    target_fps: u32,
    text: String,
    font: Font<'a>,
}

#[async_trait]
impl<FB: FrameBuffer + Sync + Send> DisplaySink<FB> for VncSink<'_, FB> {
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        statistics_tx: mpsc::Sender<StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>> {
        if !cli_args.vnc {
            return Ok(None);
        }

        let font = match cli_args.font.as_str() {
            // We ship our own copy of Arial.ttf, so that users don't need to download and provide it
            "Arial.ttf" => {
                let font_bytes = include_bytes!("../../../Arial.ttf");
                Font::try_from_bytes(font_bytes).context("failed to load default font")?
            }
            _ => {
                let font_bytes = std::fs::read(&cli_args.font)
                    .context(format!("failed to read font from file {}", cli_args.font))?;
                Font::try_from_vec(font_bytes).context(format!(
                    "failed to construct font from file {}",
                    cli_args.font
                ))?
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
            (*screen).port = cli_args.vnc_port as i32;
            (*screen).ipv6port = cli_args.vnc_port as i32;
        }

        rfb_framebuffer_malloc(screen, (fb.get_size() * 4/* bytes per pixel */) as u64);
        rfb_init_server(screen);
        rfb_run_event_loop(screen, 1, 1);

        // FIXME: Only return Some in case VNC is enabled
        Ok(Some(Self {
            fb,
            statistics_tx,
            statistics_information_rx,
            terminate_signal_rx,
            screen,
            target_fps: cli_args.fps,
            text: cli_args.text.clone(),
            font,
        }))
    }

    async fn run(&mut self) -> eyre::Result<()> {
        let vnc_fb_slice: &mut [u32] = unsafe {
            slice::from_raw_parts_mut((*self.screen).frameBuffer as *mut u32, self.fb.get_size())
        };

        // A line less because the (height - STATS_SURFACE_HEIGHT) belongs to the stats and gets refreshed by them
        let height_up_to_stats_text = self.fb.get_height() - STATS_HEIGHT - 1;
        let fb_size_up_to_stats_text = self.fb.get_width() * height_up_to_stats_text;

        let mut interval =
            time::interval(Duration::from_micros(1_000_000 / self.target_fps as u64));
        loop {
            if self.terminate_signal_rx.try_recv().is_ok() {
                return Ok(());
            }

            // I don't think we need to use spawn_blocking or something like that, as this operation should hopefully be
            // a quick memcpy. But I'm no expert on this.
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
                .send(StatisticsEvent::VncFrameRendered)
                .await
                .context(STATISTICS_SEND_ERR)?;

            if !self.statistics_information_rx.is_empty() {
                let statistics_information_event = self
                    .statistics_information_rx
                    .try_recv()
                    .context(STATISTICS_INFO_RECV_ERR)?;
                self.display_stats(statistics_information_event);
            }

            interval.tick().await;
        }
    }
}

impl<FB: FrameBuffer> VncSink<'_, FB> {
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
                "{}. {} Bit/s ({}B total) by {} connections from {} IPv6 and {} IPv4.",
                self.text,
                format_per_s(stats.bytes_per_s as f64 * 8.0),
                format(stats.bytes as f64),
                stats.connections,
                stats.ips_v6,
                stats.ips_v4,
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
