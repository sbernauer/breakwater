use std::{net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use breakwater_parser::{FrameBuffer, PixelColorBytes};
use color_eyre::eyre::{self, Context};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tracing::warn;

use crate::{
    sinks::{cli_args::SinkCliArgs, ffmpeg::FfmpegSink},
    statistics::{StatisticsEvent, StatisticsInformationEvent},
};

pub mod cli_args;
#[cfg(feature = "egui")]
pub mod egui;
pub mod ffmpeg;
#[cfg(feature = "ndi")]
pub mod ndi;
#[cfg(feature = "vnc")]
pub mod vnc;
#[cfg(feature = "winit")]
pub mod winit;

// The stabilization of async functions in traits in Rust 1.75 did not include support for using traits containing async
// functions as dyn Trait, so we still need to use async_trait here.
#[async_trait]
pub trait DisplaySink<FB> {
    async fn run(&mut self) -> eyre::Result<()>;
}

/// We can not add the `sink_type` function to [`DisplaySink`], as it needs to stay dyn compatible.
pub trait DisplaySinkType<FB>: DisplaySink<FB> {
    fn sink_type() -> Sink;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Sink {
    Ffmpeg,
    #[cfg(feature = "egui")]
    Egui,
    #[cfg(feature = "winit")]
    Winit,
    #[cfg(feature = "vnc")]
    Vnc,
    #[cfg(feature = "ndi")]
    Ndi,
}

// Several of these parameters are only consumed by feature-gated sinks, so they appear unused when those
// features are disabled. We can't use `#[expect(...)]` here, as it would fail when all features are enabled.
#[allow(unused_variables)]
pub async fn start_sinks<FB: FrameBuffer + PixelColorBytes + Send + Sync + 'static>(
    cli_args: &SinkCliArgs,
    fb: Arc<FB>,
    listen_addresses: &[SocketAddr],
    fps: u32,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
) -> eyre::Result<(Vec<JoinHandle<eyre::Result<()>>>, bool)> {
    let enabled_sinks = &cli_args.enabled_sinks;
    if enabled_sinks.is_empty() {
        warn!("No sinks enabled, not displaying/serving anything");
    }

    let (terminate_signal_tx, terminate_signal_rx) = broadcast::channel::<()>(1);

    let mut sinks = Vec::<Box<dyn DisplaySink<FB> + Send>>::new();

    let mut ffmpeg_thread_present = false;
    if enabled_sinks.contains(&FfmpegSink::<FB>::sink_type()) {
        sinks.push(Box::new(FfmpegSink::new(
            fb.clone(),
            &cli_args.ffmpeg_sink,
            fps,
            terminate_signal_rx.resubscribe(),
        )));
        ffmpeg_thread_present = true;
    }

    #[cfg(feature = "winit")]
    {
        use crate::sinks::winit::WinitSink;
        if enabled_sinks.contains(&WinitSink::<FB>::sink_type()) {
            sinks.push(Box::new(WinitSink::new(
                fb.clone(),
                terminate_signal_rx.resubscribe(),
            )));
        }
    }

    #[cfg(feature = "vnc")]
    {
        use crate::sinks::vnc::VncSink;
        if enabled_sinks.contains(&VncSink::<FB>::sink_type()) {
            sinks.push(Box::new(
                VncSink::new(
                    fb.clone(),
                    &cli_args.vnc_sink,
                    fps,
                    statistics_tx,
                    statistics_information_rx.resubscribe(),
                    terminate_signal_rx.resubscribe(),
                )
                .context("failed to create VNC sink")?,
            ));
        }
    }

    #[cfg(feature = "ndi")]
    {
        use crate::sinks::ndi::NdiSink;
        if enabled_sinks.contains(&NdiSink::<FB>::sink_type()) {
            sinks.push(Box::new(
                NdiSink::new(
                    fb.clone(),
                    &cli_args.ndi_sink,
                    fps,
                    terminate_signal_rx.resubscribe(),
                )
                .context("failed to create NDI sink")?,
            ));
        }
    }

    let mut sink_tasks = Vec::new();
    for mut sink in sinks {
        let terminate_signal_tx = terminate_signal_tx.clone();
        sink_tasks.push(tokio::spawn(async move {
            let result = sink.run().await;
            // A sink exiting - whether because it crashed or stopped normally - should bring the
            // whole server down, so signal termination to all other tasks (and the shutdown
            // handler). Best-effort: ignore the error if there are no receivers left.
            let _ = terminate_signal_tx.send(());
            result
        }));
    }

    // Egui needs some special handling around threads, so it differs a bit
    #[cfg(feature = "egui")]
    {
        use crate::sinks::egui::EguiSink;
        if enabled_sinks.contains(&EguiSink::<FB>::sink_type()) {
            let mut egui_sink = EguiSink::new(
                fb.clone(),
                &cli_args.egui_sink,
                listen_addresses,
                statistics_information_rx.resubscribe(),
                terminate_signal_rx.resubscribe(),
            )
            .context("failed to create egui sink")?;

            tokio::spawn(wait_for_shutdown(
                terminate_signal_tx.clone(),
                terminate_signal_rx,
            ));

            // Some platforms require opening windows from the main thread.
            // The tokio::main macro uses Runtime::block_on(future) which runs the future on
            // the current thread, which should be the main thread right now.
            egui_sink.run().await.context("failed to run egui sink")?;

            // The egui window was closed - bring the rest of the server down too.
            let _ = terminate_signal_tx.send(());
        } else {
            wait_for_shutdown(terminate_signal_tx, terminate_signal_rx).await?;
        }
    }

    #[cfg(not(feature = "egui"))]
    wait_for_shutdown(terminate_signal_tx, terminate_signal_rx).await?;

    Ok((sink_tasks, ffmpeg_thread_present))
}

/// Blocks until either the user requests a shutdown via Ctrl+C, or one of the sinks signals
/// termination (e.g. because it crashed). Afterwards all remaining sinks are told to terminate.
async fn wait_for_shutdown(
    terminate_signal_tx: broadcast::Sender<()>,
    mut terminate_signal_rx: broadcast::Receiver<()>,
) -> eyre::Result<()> {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("failed to wait for ctrl + c")?;
        }
        // A sink signalled termination, e.g. because it crashed.
        _ = terminate_signal_rx.recv() => {}
    }

    // Tell all remaining sinks to terminate. Best-effort: this fails if all other receivers are
    // already gone (e.g. the only sink crashed and dropped its receiver), which is fine here.
    let _ = terminate_signal_tx.send(());

    Ok(())
}
