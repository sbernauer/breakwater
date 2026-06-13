//! `deich` is breakwater's distributed mode: a fleet of **workers**, each running a Pixelflut
//! server, that continuously sync their framebuffer to a central **collector** which merges them
//! into the final picture.
//!
//! A single binary runs either role (see [`cli_args::Role`]). Right now only the [`worker`] is
//! functional. The [`sync`] protocol is still just a raw framebuffer blob, and the [`collector`]
//! is not implemented yet — both are next on the list.

use color_eyre::eyre;

pub mod cli_args;
pub mod collector;
pub mod sync;
pub mod worker;

/// Shared process setup for the `deich` binaries: error reporting and logging.
pub fn init_telemetry() -> eyre::Result<()> {
    color_eyre::install()?;

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(if cfg!(debug_assertions) {
            tracing::Level::DEBUG.into()
        } else {
            tracing::Level::INFO.into()
        })
        .from_env()?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    Ok(())
}
