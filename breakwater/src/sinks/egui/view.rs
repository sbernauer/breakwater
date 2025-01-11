use std::sync::Arc;

use breakwater_parser::FrameBuffer;
use eframe::egui_glow;
use snafu::OptionExt;
use tokio::sync::broadcast;

use crate::statistics::StatisticsInformationEvent;

use super::{
    canvas_renderer::{CanvasRenderer, Vertex},
    dynamic_overlay::UiOverlay,
    ViewportConfig,
};

pub struct EguiView<FB: FrameBuffer> {
    framebuffer: Arc<FB>,
    canvas_renderer: Arc<CanvasRenderer<FB>>,
    viewports: Vec<ViewportConfig>,
    terminate_rx: broadcast::Receiver<()>,
    stats_rx: broadcast::Receiver<StatisticsInformationEvent>,
    advertised_endpoints: Vec<String>,

    ui: Arc<UiOverlay>,
    latest_stats: StatisticsInformationEvent,
}

impl<FB: FrameBuffer + Send + Sync + 'static> EguiView<FB> {
    pub fn new<'a>(
        cc: &'a eframe::CreationContext<'a>,
        framebuffer: Arc<FB>,
        viewports: Vec<ViewportConfig>,
        terminate_rx: broadcast::Receiver<()>,
        stats_rx: broadcast::Receiver<StatisticsInformationEvent>,
        advertised_endpoints: Vec<String>,
        ui: Arc<UiOverlay>,
    ) -> Result<Self, super::Error> {
        let gl_context = cc
            .gl
            .as_ref()
            .context(super::UnsupportedEguiFrontendSnafu)?;

        let canvas_renderer = CanvasRenderer::new(
            gl_context,
            framebuffer.clone(),
            viewports.len().try_into().expect("at least one viewport"),
        );
        let canvas_renderer = Arc::new(canvas_renderer);

        Ok(Self {
            latest_stats: StatisticsInformationEvent::default(),
            ui,

            framebuffer,
            viewports,
            canvas_renderer,
            terminate_rx,
            stats_rx,
            advertised_endpoints,
        })
    }

    fn draw_canvas(&self, ctx: &egui::Context, view_port_index: usize, view_port: ViewportConfig) {
        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());

        let canvas_renderer = self.canvas_renderer.clone();
        let framebuffer = self.framebuffer.clone();

        let callback = egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let new_vertices = calc_new_vertices(
                    &view_port,
                    [
                        info.viewport_in_pixels().width_px,
                        info.viewport_in_pixels().height_px,
                    ],
                    [framebuffer.get_width(), framebuffer.get_height()],
                );

                canvas_renderer.prepare(painter.gl(), view_port_index, Some(new_vertices));
                canvas_renderer.paint(painter.gl(), view_port_index);
            })),
        };

        painter.add(callback);
    }
}

/// calculates vertices that the canvas keeps its aspect ratio, but is resized to fit onto the given viewport
fn calc_new_vertices(
    canvas_view_port: &ViewportConfig,
    [pixel_width, pixel_height]: [i32; 2],
    [canvas_width, canvas_height]: [usize; 2],
) -> [Vertex; 4] {
    let mut a = 1f32;
    let mut b = 1f32;

    if pixel_width as f32 / pixel_height as f32
        > canvas_view_port.width as f32 / canvas_view_port.height as f32
    {
        a = (pixel_height as f32 / pixel_width as f32)
            * (canvas_view_port.width as f32 / canvas_view_port.height as f32);
    } else {
        b = (pixel_width as f32 / pixel_height as f32)
            * (canvas_view_port.height as f32 / canvas_view_port.width as f32);
    }

    let u = canvas_view_port.x as f32 / canvas_width as f32;
    let uu = canvas_view_port.width as f32 / canvas_width as f32;
    let v = canvas_view_port.y as f32 / canvas_height as f32;
    let vv = canvas_view_port.height as f32 / canvas_height as f32;

    [
        Vertex {
            position: [-1.0 * a, 1.0 * b],
            tex_coords: [u, v],
        },
        Vertex {
            position: [-1.0 * a, -1.0 * b],
            tex_coords: [u, v + vv],
        },
        Vertex {
            position: [1.0 * a, 1.0 * b],
            tex_coords: [u + uu, v],
        },
        Vertex {
            position: [1.0 * a, -1.0 * b],
            tex_coords: [u + uu, v + vv],
        },
    ]
}

impl<FB: FrameBuffer + Send + Sync + 'static> eframe::App for EguiView<FB> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        match self.terminate_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {}
            _ => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        };

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
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }

        for (i, vp) in self.viewports.iter().copied().enumerate() {
            if i == 0 {
                // first view port on main window
                self.draw_canvas(ctx, i, vp);
                self.ui.draw_ui(
                    i as u32,
                    ctx,
                    &self.advertised_endpoints,
                    self.latest_stats.connections,
                    self.latest_stats.ips,
                    self.latest_stats.legacy_ips,
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

                        self.draw_canvas(ctx, i, vp);
                        self.ui.draw_ui(
                            i as u32,
                            ctx,
                            &self.advertised_endpoints,
                            self.latest_stats.connections,
                            self.latest_stats.ips,
                            self.latest_stats.legacy_ips,
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
