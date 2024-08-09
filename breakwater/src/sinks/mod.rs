use std::sync::Arc;

use async_trait::async_trait;
use snafu::Snafu;
use tokio::sync::{broadcast, mpsc};

use crate::{
    cli_args::CliArgs,
    statistics::{StatisticsEvent, StatisticsInformationEvent},
};

pub mod ffmpeg;
#[cfg(feature = "vnc")]
pub mod vnc;

#[derive(Debug, Snafu)]
pub enum Error {
    #[cfg(feature = "vnc")]
    #[snafu(display("VNC error"), context(false))]
    VncError { source: vnc::Error },

    #[snafu(display("ffmpeg error"), context(false))]
    FfmpegError { source: ffmpeg::Error },
}

// The stabilization of async functions in traits in Rust 1.75 did not include support for using traits containing async
//functions as dyn Trait, so we still need to use async_trait here.
#[async_trait]
pub trait DisplaySink<FB> {
    /// This function can return [`None`] in case this sink is not configured (by looking at the `cli_args`).
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        statistics_tx: mpsc::Sender<StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> Result<Option<Self>, Error>
    where
        Self: Sized;

    async fn run(&mut self) -> Result<(), Error>;
}
