use std::sync::Arc;

use breakwater_parser::{FrameBuffer, PixelColorBytes};
use egui::{Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use tokio::sync::broadcast;

use super::{ViewportConfig, dynamic_overlay::UiOverlay};
use crate::statistics::StatisticsInformationEvent;

pub struct EguiView<FB: FrameBuffer + PixelColorBytes> {
    canvas_texture: TextureHandle,

    fb: Arc<FB>,
    viewports: Vec<ViewportConfig>,
    terminate_rx: broadcast::Receiver<()>,
    stats_rx: broadcast::Receiver<StatisticsInformationEvent>,
    advertised_endpoints: Vec<String>,

    ui: Arc<UiOverlay>,
    latest_stats: StatisticsInformationEvent,
}

impl<FB: FrameBuffer + PixelColorBytes + Send + Sync + 'static> EguiView<FB> {
    pub fn new<'a>(
        cc: &'a eframe::CreationContext<'a>,
        fb: Arc<FB>,
        viewports: Vec<ViewportConfig>,
        terminate_rx: broadcast::Receiver<()>,
        stats_rx: broadcast::Receiver<StatisticsInformationEvent>,
        advertised_endpoints: Vec<String>,
        ui: Arc<UiOverlay>,
    ) -> Self {
        let canvas_texture_id = cc.egui_ctx.tex_manager().write().alloc(
            "canvas texture".into(),
            ColorImage::filled([fb.get_width(), fb.get_height()], Color32::BLACK).into(),
            TextureOptions::NEAREST,
        );
        let canvas_texture = TextureHandle::new(cc.egui_ctx.tex_manager(), canvas_texture_id);

        Self {
            canvas_texture,

            fb,
            viewports,
            terminate_rx,
            stats_rx,
            advertised_endpoints,

            ui,
            latest_stats: StatisticsInformationEvent::default(),
        }
    }

    fn draw_canvas(&self, ctx: &egui::Context, view_port: ViewportConfig) {
        // get egui background painter
        let bg = ctx.layer_painter(egui::LayerId::background());
        let bg_rect = bg.clip_rect();
        let bg_ratio = bg_rect.aspect_ratio();

        let vp_rect = Rect::from_min_size(
            Pos2::new(view_port.x as _, view_port.y as _),
            Vec2::new(view_port.width as _, view_port.height as _),
        );
        let vp_ratio = vp_rect.aspect_ratio();

        // determine how to shrink the area we draw to
        // to keep the correct aspect ratio
        let mut w_shrink = 1.0;
        let mut h_shrink = 1.0;

        if vp_ratio > bg_ratio {
            h_shrink = bg_ratio / vp_ratio;
        } else {
            w_shrink = vp_ratio / bg_ratio;
        }

        let fb_w = self.fb.get_width() as f32;
        let fb_h = self.fb.get_height() as f32;
        let vp_uv = Rect::from_two_pos(
            Pos2::new(vp_rect.min.x / fb_w, vp_rect.min.y / fb_h),
            Pos2::new(vp_rect.max.x / fb_w, vp_rect.max.y / fb_h),
        );

        let draw_rect = Rect::from_center_size(
            bg_rect.center(),
            Vec2::new(bg_rect.width() * w_shrink, bg_rect.height() * h_shrink),
        );

        bg.image(self.canvas_texture.id(), draw_rect, vp_uv, Color32::WHITE);
    }
}

impl<FB: FrameBuffer + PixelColorBytes + Send + Sync + 'static> eframe::App for EguiView<FB> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx();
        #[allow(clippy::single_match_else)]
        match self.terminate_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {}
            _ => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        loop {
            match self.stats_rx.try_recv() {
                Err(broadcast::error::TryRecvError::Empty) => break,
                Ok(stats) => {
                    self.latest_stats = stats;
                    break;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    unreachable!("where stats?");
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            }
        }

        self.canvas_texture.set(
            ColorImage::from_rgba_unmultiplied(
                [self.fb.get_width(), self.fb.get_height()],
                self.fb.pixel_color_bytes(),
            ),
            TextureOptions::NEAREST,
        );

        for (i, vp) in self.viewports.iter().copied().enumerate() {
            if i == 0 {
                // first view port on main window
                self.draw_canvas(ctx, vp);
                self.ui.draw_ui(
                    i as u32,
                    ctx,
                    &self.advertised_endpoints,
                    self.latest_stats.connections,
                    self.latest_stats.ips_v6,
                    self.latest_stats.ips_v4,
                    self.latest_stats.bytes_per_s,
                );
            } else {
                let child_requested_close = ctx.show_viewport_immediate(
                    egui::ViewportId::from_hash_of(format!("viewport-{i}")),
                    egui::ViewportBuilder::default()
                        .with_title(format!("Viewport {i} | Breakwater")),
                    |ctx, class| {
                        assert!(
                            class == egui::ViewportClass::Immediate,
                            "This egui backend doesn't support multiple viewports"
                        );

                        if ctx.input(|i| i.viewport().close_requested()) {
                            // should close
                            return true;
                        }

                        self.draw_canvas(ctx, vp);
                        self.ui.draw_ui(
                            i as u32,
                            ctx,
                            &self.advertised_endpoints,
                            self.latest_stats.connections,
                            self.latest_stats.ips_v6,
                            self.latest_stats.ips_v4,
                            self.latest_stats.bytes_per_s,
                        );

                        // should close
                        false
                    },
                );

                if child_requested_close {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        ctx.request_repaint();
    }
}
