use async_trait::async_trait;
use color_eyre::eyre;

#[cfg(feature = "egui")]
pub mod egui;
pub mod ffmpeg;
#[cfg(all(feature = "native-display", not(feature = "egui")))]
pub mod native_display;
#[cfg(feature = "ndi")]
pub mod ndi;
#[cfg(feature = "vnc")]
pub mod vnc;

// The stabilization of async functions in traits in Rust 1.75 did not include support for using traits containing async
// functions as dyn Trait, so we still need to use async_trait here.
//
// Each sink has its own inherent `new(..)` constructor (their arguments differ), returning
// `Ok(None)` when the sink isn't configured. This trait only carries the shared run loop.
#[async_trait]
pub trait DisplaySink<FB> {
    async fn run(&mut self) -> eyre::Result<()>;
}
