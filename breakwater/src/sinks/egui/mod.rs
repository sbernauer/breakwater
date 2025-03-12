use std::{fmt::Display, net::ToSocketAddrs, str::FromStr, sync::Arc};

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use color_eyre::eyre::{self, Context};
use dynamic_overlay::UiOverlay;
use log::error;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use super::DisplaySink;
use crate::statistics::StatisticsInformationEvent;

mod canvas_renderer;
mod dynamic_overlay;
mod view;

/// Describes the part of the framebuffer that the corresponding viewport will display.
#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct ViewportConfig {
    /// x offset
    pub x: usize,
    /// y offset
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct InvalidViewportConfig;
impl Display for InvalidViewportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid view port config")
    }
}
impl std::error::Error for InvalidViewportConfig {}

impl FromStr for ViewportConfig {
    type Err = InvalidViewportConfig;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Result<Vec<_>, _> = s
            .split(",")
            .flat_map(|s| s.split("x"))
            .map(usize::from_str)
            .collect();

        match parts {
            Err(e) => {
                error!("failed to parse view port config: {e}");
                Err(InvalidViewportConfig)
            }
            Ok(parts) if parts.len() != 4 => {
                error!("failed to parse view port config: invalid format");
                Err(InvalidViewportConfig)
            }
            Ok(parts) => Ok(Self {
                x: parts[0],
                y: parts[1],
                width: parts[2],
                height: parts[3],
            }),
        }
    }
}

pub struct EguiSink<FB: FrameBuffer> {
    framebuffer: Arc<FB>,
    viewports: Vec<ViewportConfig>,
    terminate_rx: broadcast::Receiver<()>,
    stats_rx: broadcast::Receiver<StatisticsInformationEvent>,
    advertised_endpoints: Vec<String>,
    ui_overlay: Arc<UiOverlay>,
}

#[async_trait]
impl<FB: FrameBuffer + Send + Sync + 'static> DisplaySink<FB> for EguiSink<FB> {
    /// This function can return [`None`] in case this sink is not configured (by looking at the `cli_args`).
    async fn new(
        fb: Arc<FB>,
        cli_args: &crate::cli_args::CliArgs,
        _statistics_tx: mpsc::Sender<crate::statistics::StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>>
    where
        Self: Sized,
    {
        let viewports = match (
            cli_args.viewport.as_slice(),
            cli_args.native_display,
            cli_args.ui.as_ref(),
        ) {
            (vp, _, _) if !vp.is_empty() => Vec::from(vp),
            ([], _, Some(_)) | ([], true, _) => vec![ViewportConfig {
                x: 0,
                y: 0,
                width: fb.get_width(),
                height: fb.get_height(),
            }],
            _ => return Ok(None),
        };

        let ui_overlay = Arc::new({
            if let Some(ui) = cli_args.ui.as_ref() {
                dynamic_overlay::load_and_check(ui).context("failed to load dynamic overlay")?
            } else {
                UiOverlay::BuiltIn
            }
        });

        let mut advertised_endpoints = cli_args.advertised_endpoints.clone();
        if advertised_endpoints.is_empty() {
            let port = cli_args
                .listen_address
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap()
                .port();
            if let Ok(local_ip) = local_ip_address::local_ip() {
                advertised_endpoints.push(format!("{local_ip}:{port}"));
            }
            if let Ok(local_ip) = local_ip_address::local_ipv6() {
                advertised_endpoints.push(format!("[{local_ip}]:{port}"));
            }
        }

        Ok(Some(Self {
            framebuffer: fb,
            viewports,
            terminate_rx: terminate_signal_rx,
            stats_rx: statistics_information_rx,
            advertised_endpoints,
            ui_overlay,
        }))
    }

    /// This should only run on the main thread
    async fn run(&mut self) -> eyre::Result<()> {
        // block_in_place below should only be used in a MultiThread runtime
        assert_eq!(
            tokio::runtime::Handle::current().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        );

        tokio::task::block_in_place(move || {
            if let Err(e) = self.run_eframe_display() {
                eyre::bail!("egui failed: {e:?}");
            }

            Ok(())
        })
    }
}

impl<FB: FrameBuffer + Send + Sync + 'static> EguiSink<FB> {
    fn run_eframe_display(&self) -> Result<(), eframe::Error> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default(),
            renderer: eframe::Renderer::Glow,
            window_builder: Some(Box::new(|builder| builder.with_app_id("breakwater"))),
            ..Default::default()
        };

        let terminate = self.terminate_rx.resubscribe();
        let stats = self.stats_rx.resubscribe();
        let framebuffer = self.framebuffer.clone();
        let viewports = self.viewports.clone();
        let advertised_endpoints = self.advertised_endpoints.clone();
        let ui_overlay = self.ui_overlay.clone();

        eframe::run_native(
            "Viewport 0 | Breakwater",
            options,
            Box::new(|cc| {
                let frontend = view::EguiView::new(
                    cc,
                    framebuffer,
                    viewports,
                    terminate,
                    stats,
                    advertised_endpoints,
                    ui_overlay,
                )
                .expect("failed to create egui frontend");

                Ok(Box::new(frontend))
            }),
        )
    }
}
