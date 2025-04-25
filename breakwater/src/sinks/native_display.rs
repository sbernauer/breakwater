use std::{num::NonZero, sync::Arc};

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use color_eyre::eyre::{self, Context};
use tokio::sync::{broadcast, mpsc};
use tracing::instrument;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{self, EventLoop},
    platform::wayland::EventLoopBuilderExtWayland,
    raw_window_handle::{DisplayHandle, HasDisplayHandle},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    cli_args::CliArgs,
    sinks::DisplaySink,
    statistics::{StatisticsEvent, StatisticsInformationEvent},
};

// Sorry! Help needed :)
unsafe impl<FB: FrameBuffer> Send for NativeDisplaySink<FB> {}

pub struct NativeDisplaySink<FB: FrameBuffer> {
    fb: Arc<FB>,
    terminate_signal_rx: broadcast::Receiver<()>,

    surface: Option<softbuffer::Surface<DisplayHandle<'static>, Arc<Window>>>,
}

#[async_trait]
impl<FB: FrameBuffer + Sync + Send + 'static> DisplaySink<FB> for NativeDisplaySink<FB> {
    #[instrument(skip_all, err)]
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        _statistics_tx: mpsc::Sender<StatisticsEvent>,
        _statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>> {
        if !cli_args.native_display {
            return Ok(None);
        }

        Ok(Some(Self {
            fb,
            terminate_signal_rx,
            surface: None,
        }))
    }

    #[instrument(skip(self), err)]
    async fn run(&mut self) -> eyre::Result<()> {
        let fb_clone = self.fb.clone();
        let terminate_signal_rx = self.terminate_signal_rx.resubscribe();

        tokio::task::spawn_blocking(move || {
            // We need a owned self, so let's re-create one
            let mut self_clone = Self {
                fb: fb_clone,
                terminate_signal_rx,
                surface: None,
            };

            let event_loop = EventLoop::builder()
                // FIXME: Can we get rid of this?
                .with_any_thread(true)
                .build()
                .context("failed to create event loop")?;

            event_loop
                .run_app(&mut self_clone)
                .context("failed to run event loop")?;

            eyre::Result::<()>::Ok(())
        })
        .await
        .context("failed to join native display thread")??;

        Ok(())
    }
}

impl<FB: FrameBuffer> ApplicationHandler for NativeDisplaySink<FB> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(self.window_attributes())
                .context("failed to create window")
                .unwrap(),
        );
        self.surface = {
            let context = softbuffer::Context::new(unsafe {
                // Fiddling around with lifetimes
                std::mem::transmute::<DisplayHandle, DisplayHandle>(
                    event_loop
                        .display_handle()
                        .context("failed to get display handle")
                        .unwrap(),
                )
            })
            .expect("Failed to create window context");
            Some(softbuffer::Surface::new(&context, window).expect("Failed to create surface"))
        };
    }

    fn window_event(
        &mut self,
        event_loop: &event_loop::ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.terminate_signal_rx.try_recv().is_ok() {
            event_loop.exit();
            return;
        }

        let Some(surface) = self.surface.as_mut() else {
            return;
        };

        match event {
            WindowEvent::Resized(_size) => {
                surface
                    .resize(
                        NonZero::new(self.fb.get_width() as u32).unwrap(),
                        NonZero::new(self.fb.get_height() as u32).unwrap(),
                    )
                    .expect("Failed to resize surface");
                surface.window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let window = surface.window().clone();
                let mut buffer = surface.buffer_mut().expect("Failed to get mutable buffer");

                let fb_size = self.fb.get_size();
                if buffer.len() != fb_size {
                    tracing::warn!(
                        buffer_size = buffer.len(),
                        fb_size,
                        "window buffer has size {}, but fb has size {}! Skipping redraw.",
                        buffer.len(),
                        fb_size
                    );
                    return;
                }

                buffer.copy_from_slice(
                    &self
                        .fb
                        .as_bytes()
                        .chunks_exact(4)
                        .map(|chunk| u32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]))
                        .collect::<Vec<_>>(),
                );

                window.pre_present_notify();
                buffer.present().expect("Failed to present buffer");
                window.request_redraw();
            }
            WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. } => (),
            _ => {
                tracing::debug!(?event, "received window event");
            }
        };
    }
}

impl<FB: FrameBuffer> NativeDisplaySink<FB> {
    fn window_attributes(&self) -> WindowAttributes {
        Window::default_attributes()
            .with_title("Pixelflut server (breakwater)")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.fb.get_width() as u32,
                self.fb.get_height() as u32,
            ))
    }
}
