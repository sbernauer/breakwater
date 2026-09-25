use std::sync::Arc;

use breakwater_parser::{FrameBuffer, PixelColorBytes};
use eframe::wgpu::{TexelCopyBufferLayout, TexelCopyTextureInfo, Texture};
use egui::{Color32, ColorImage, LayerId, Pos2, Rect, TextureId, TextureOptions, Vec2};
use tokio::sync::broadcast;

use super::{ViewportConfig, dynamic_overlay::UiOverlay};
use crate::statistics::StatisticsInformationEvent;

pub struct EguiView<FB: FrameBuffer + PixelColorBytes> {
    canvas_texture: Texture,
    canvas_texture_id: TextureId,

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
        let render_state = cc.wgpu_render_state.as_ref().expect("wgpu renderer active");

        let canvas_texture_id;
        {
            let tex_manager = cc.egui_ctx.tex_manager();
            let mut tex_manager = tex_manager.write();

            canvas_texture_id = tex_manager.alloc(
                "canvas texture".into(),
                ColorImage::filled([fb.get_width(), fb.get_height()], Color32::BLACK).into(),
                TextureOptions::NEAREST,
            );

            let mut delta = tex_manager.take_delta();

            let mut renderer = render_state.renderer.write();
            for id in delta.free.drain() {
                renderer.free_texture(&id);
            }
            for (id, deltas) in delta.set.drain() {
                for delta in deltas {
                    if delta.image.width() != 0 && delta.image.height() != 0 {
                        renderer.update_texture(
                            &render_state.device,
                            &render_state.queue,
                            id,
                            &delta,
                        );
                    }
                }
            }
        }

        let canvas_texture = render_state
            .renderer
            .read()
            .texture(&canvas_texture_id)
            .expect("where did our texture go??")
            .texture
            .as_ref()
            .expect("this should be a real texture")
            .clone();

        Self {
            canvas_texture_id,
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

        // clear viewport
        bg.rect_filled(bg_rect, 0.0, Color32::BLACK);
        bg.image(self.canvas_texture_id, draw_rect, vp_uv, Color32::WHITE);
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

        ui.layer_painter(LayerId::background()).add(
            eframe::egui_wgpu::Callback::new_paint_callback(
                ui.clip_rect(),
                CanvasUpload {
                    fb: self.fb.clone(),
                    texture: self.canvas_texture.clone(),
                },
            ),
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

struct CanvasUpload<FB> {
    texture: Texture,
    fb: Arc<FB>,
}

impl<FB: FrameBuffer + PixelColorBytes + Sync + Send> eframe::egui_wgpu::CallbackTrait
    for CanvasUpload<FB>
{
    fn prepare(
        &self,
        _device: &eframe::wgpu::Device,
        queue: &eframe::wgpu::Queue,
        _screen_descriptor: &eframe::egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut eframe::wgpu::CommandEncoder,
        _callback_resources: &mut eframe::egui_wgpu::CallbackResources,
    ) -> Vec<eframe::wgpu::CommandBuffer> {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: eframe::wgpu::Origin3d::ZERO,
                aspect: eframe::wgpu::TextureAspect::All,
            },
            self.fb.pixel_color_bytes(),
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.fb.get_width() as u32 * 4 /* bytes per texel */),
                rows_per_image: Some(self.fb.get_height() as _),
            },
            eframe::wgpu::Extent3d {
                width: self.fb.get_width() as _,
                height: self.fb.get_height() as _,
                depth_or_array_layers: 1,
            },
        );

        vec![]
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut eframe::wgpu::RenderPass<'static>,
        _callback_resources: &eframe::egui_wgpu::CallbackResources,
    ) {
    }
}
