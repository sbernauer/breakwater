use std::{num::NonZero, sync::Arc};

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use snafu::{ResultExt, Snafu};
use softbuffer::{Context, Surface};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinError,
};
use winit::{
    application::ApplicationHandler,
    error::EventLoopError,
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

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to join native display thread"))]
    JoinNativeDisplayThread { source: JoinError },

    #[snafu(display("Failed to build eventloop"))]
    BuildEventLoop { source: EventLoopError },

    #[snafu(display("Failed to run eventloop"))]
    RunEventLoop { source: EventLoopError },
}

// Sorry! Help needed :)
unsafe impl<'a, FB: FrameBuffer> Send for NativeDisplaySink<FB> {}

pub struct NativeDisplaySink<FB: FrameBuffer> {
    fb: Arc<FB>,
    terminate_signal_rx: broadcast::Receiver<()>,

    surface: Option<Surface<DisplayHandle<'static>, Arc<Window>>>,
}

#[async_trait]
impl<FB: FrameBuffer + Sync + Send + 'static> DisplaySink<FB> for NativeDisplaySink<FB> {
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        _statistics_tx: mpsc::Sender<StatisticsEvent>,
        _statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> Result<Option<Self>, super::Error> {
        if !cli_args.native_display {
            return Ok(None);
        }

        Ok(Some(Self {
            fb,
            terminate_signal_rx,
            surface: None,
        }))
    }

    async fn run(&mut self) -> Result<(), super::Error> {
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
                .context(BuildEventLoopSnafu)?;

            event_loop
                .run_app(&mut self_clone)
                .context(RunEventLoopSnafu)?;

            Ok::<(), super::Error>(())
        })
        .await
        .context(JoinNativeDisplayThreadSnafu)??;

        Ok(())
    }
}

impl<FB: FrameBuffer> ApplicationHandler for NativeDisplaySink<FB> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window = Arc::new(event_loop.create_window(self.window_attributes()).unwrap());
        self.surface = {
            let context =
                Context::new(unsafe { std::mem::transmute(event_loop.display_handle().unwrap()) })
                    .unwrap();
            Some(Surface::new(&context, window).unwrap())
        };
    }

    fn window_event(
        &mut self,
        event_loop: &event_loop::ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };

        if self.terminate_signal_rx.try_recv().is_ok() {
            event_loop.exit();
        }

        match event {
            WindowEvent::Resized(_size) => {
                surface
                    .resize(
                        NonZero::new(self.fb.get_width() as u32).unwrap(),
                        NonZero::new(self.fb.get_height() as u32).unwrap(),
                    )
                    .unwrap();
                surface.window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let window = surface.window().clone();
                let mut buffer = surface.buffer_mut().unwrap();

                buffer.copy_from_slice(
                    &self
                        .fb
                        .as_pixels()
                        .iter()
                        .map(|pixel| (pixel << 8).swap_bytes())
                        .collect::<Vec<_>>(),
                );
                window.pre_present_notify();
                buffer.present().unwrap();
                window.request_redraw();
            }
            WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. } => (),
            _ => {
                log::debug!("Window={:?}", event);
            }
        };
    }
}

impl<FB: FrameBuffer> NativeDisplaySink<FB> {
    fn window_attributes(&self) -> WindowAttributes {
        Window::default_attributes()
            .with_title("Pixelflut server (breakwater)")
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.fb.get_width() as u32,
                self.fb.get_height() as u32,
            ))
    }
}
