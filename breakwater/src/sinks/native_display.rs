use breakwater_parser::FrameBuffer;
use snafu::{ResultExt, Snafu};
use softbuffer::{Context, Surface};
use std::{num::NonZero, sync::Arc};
use winit::{
    application::ApplicationHandler,
    error::EventLoopError,
    event::WindowEvent,
    event_loop::{self, EventLoop},
    raw_window_handle::{DisplayHandle, HasDisplayHandle},
    window::{Window, WindowAttributes, WindowId},
};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to build eventloop"))]
    BuildEventLoop { source: EventLoopError },

    #[snafu(display("Failed to run eventloop"))]
    RunEventLoop { source: EventLoopError },
}

pub struct DisplaySink<FB: FrameBuffer> {
    surface: Option<Surface<DisplayHandle<'static>, Arc<Window>>>,
    fb: Arc<FB>,
}

impl<FB: FrameBuffer> DisplaySink<FB> {
    pub fn new(fb: Arc<FB>) -> Result<(EventLoop<()>, Self), Error> {
        let event_loop = EventLoop::builder().build().context(BuildEventLoopSnafu)?;
        Ok((event_loop, DisplaySink { surface: None, fb }))
    }

    pub fn run(&mut self, event_loop: EventLoop<()>) -> Result<(), Error> {
        event_loop.run_app(self).context(RunEventLoopSnafu)
    }
}

impl<FB: FrameBuffer> ApplicationHandler for DisplaySink<FB> {
    fn user_event(&mut self, event_loop: &event_loop::ActiveEventLoop, _event: ()) {
        event_loop.exit();
    }

    fn window_event(
        &mut self,
        _event_loop: &event_loop::ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
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

    fn resumed(&mut self, event_loop: &event_loop::ActiveEventLoop) {
        let window = Arc::new(event_loop.create_window(self.window_attributes()).unwrap());
        self.surface = {
            let context =
                Context::new(unsafe { std::mem::transmute(event_loop.display_handle().unwrap()) })
                    .unwrap();
            Some(Surface::new(&context, window).unwrap())
        };
    }
}

impl<FB: FrameBuffer> DisplaySink<FB> {
    fn window_attributes(&self) -> WindowAttributes {
        Window::default_attributes()
            .with_title("Pixelflut server (breakwater)")
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.fb.get_width() as u32,
                self.fb.get_height() as u32,
            ))
    }
}
