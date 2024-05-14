use std::{num::TryFromIntError, sync::Arc};

use breakwater_core::framebuffer::FrameBuffer;
use clap::Parser;
use env_logger::Env;
use prometheus_exporter::PrometheusExporter;
use sinks::ffmpeg::FfmpegSink;
use snafu::{ResultExt, Snafu};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{
    cli_args::CliArgs,
    server::Server,
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};

#[cfg(feature = "vnc")]
use {
    crate::sinks::vnc::{self, VncServer},
    thread_priority::{ThreadBuilderExt, ThreadPriority},
};

mod cli_args;
mod prometheus_exporter;
mod server;
mod sinks;
mod statistics;

#[cfg(test)]
mod tests;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to start Pixelflut server"))]
    StartPixelflutServer { source: server::Error },

    #[snafu(display("Failed to wait for CTRL + C signal"))]
    WaitForCtrlCSignal { source: std::io::Error },

    #[snafu(display("Failed to start Prometheus exporter"))]
    StartPrometheusExporter { source: prometheus_exporter::Error },

    #[snafu(display("Invalid network buffer size {network_buffer_size:?}"))]
    InvalidNetworkBufferSize {
        source: TryFromIntError,
        network_buffer_size: i64,
    },

    #[cfg(feature = "vnc")]
    #[snafu(display("Failed to spawn VNC server thread"))]
    SpawnVncServerThread { source: std::io::Error },

    #[cfg(feature = "vnc")]
    #[snafu(display("Failed to send VNC server shutdown signal"))]
    SendVncServerShutdownSignal {},

    #[cfg(feature = "vnc")]
    #[snafu(display("Failed to stop VNC server thread"))]
    StopVncServerThread {},

    #[cfg(feature = "vnc")]
    #[snafu(display("Failed to start VNC server"))]
    StartVncServer { source: vnc::Error },

    #[cfg(feature = "vnc")]
    #[snafu(display("Failed to get cross-platform ThreadPriority. Please report this error message together with your operating system: {message}"))]
    GetThreadPriority { message: String },
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();
    let args = CliArgs::parse();

    let fb = Arc::new(FrameBuffer::new(args.width, args.height));

    // If we make the channel to big, stats will start to lag behind
    // TODO: Check performance impact in real-world scenario. Maybe the statistics thread blocks the other threads
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (statistics_information_tx, statistics_information_rx_for_prometheus_exporter) =
        broadcast::channel::<StatisticsInformationEvent>(2);
    let (ffmpeg_terminate_signal_tx, ffmpeg_terminate_signal_rx) = oneshot::channel();

    #[cfg(feature = "vnc")]
    let (vnc_terminate_signal_tx, vnc_terminate_signal_rx) = oneshot::channel();
    #[cfg(feature = "vnc")]
    let statistics_information_rx_for_vnc_server = statistics_information_tx.subscribe();

    let statistics_save_mode = if args.disable_statistics_save_file {
        StatisticsSaveMode::Disabled
    } else {
        StatisticsSaveMode::Enabled {
            save_file: args.statistics_save_file.clone(),
            interval_s: args.statistics_save_interval_s,
        }
    };
    let mut statistics = Statistics::new(
        statistics_rx,
        statistics_information_tx,
        statistics_save_mode,
    );

    let server = Server::new(
        &args.listen_address,
        Arc::clone(&fb),
        statistics_tx.clone(),
        args.network_buffer_size
            .try_into()
            // This should never happen as clap checks the range for us
            .context(InvalidNetworkBufferSizeSnafu {
                network_buffer_size: args.network_buffer_size,
            })?,
    )
    .await
    .context(StartPixelflutServerSnafu)?;
    let mut prometheus_exporter = PrometheusExporter::new(
        &args.prometheus_listen_address,
        statistics_information_rx_for_prometheus_exporter,
    )
    .context(StartPrometheusExporterSnafu)?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let statistics_thread = tokio::spawn(async move { statistics.start().await });
    let prometheus_exporter_thread = tokio::spawn(async move { prometheus_exporter.run().await });

    let ffmpeg_sink = FfmpegSink::new(&args, Arc::clone(&fb));
    let ffmpeg_thread = ffmpeg_sink.map(|sink| {
        tokio::spawn(async move { sink.run(ffmpeg_terminate_signal_rx).await.unwrap() })
    });

    #[cfg(feature = "vnc")]
    let vnc_server_thread = {
        let fb_for_vnc_server = Arc::clone(&fb);
        let mut vnc_server = VncServer::new(
            fb_for_vnc_server,
            args.vnc_port,
            args.fps,
            statistics_tx,
            statistics_information_rx_for_vnc_server,
            vnc_terminate_signal_rx,
            args.text,
            args.font,
        )
        .context(StartVncServerSnafu)?;

        // TODO Use tokio::spawn instead of std::thread::spawn
        // I was not able to get to work with async closure
        // We than also need to think about setting a priority
        std::thread::Builder::new()
            .name("breakwater vnc server thread".to_owned())
            .spawn_with_priority(
                ThreadPriority::Crossplatform(70.try_into().map_err(|err: &str| {
                    Error::GetThreadPriority {
                        message: err.to_string(),
                    }
                })?),
                move |_| vnc_server.run().context(StartVncServerSnafu),
            )
    }
    .context(SpawnVncServerThreadSnafu)?;

    tokio::signal::ctrl_c()
        .await
        .context(WaitForCtrlCSignalSnafu)?;

    prometheus_exporter_thread.abort();
    server_listener_thread.abort();
    statistics_thread.abort();
    if let Some(ffmpeg_thread) = ffmpeg_thread {
        let _ = ffmpeg_terminate_signal_tx.send(());
        ffmpeg_thread.abort();
    }

    #[cfg(feature = "vnc")]
    {
        vnc_terminate_signal_tx
            .send("bye bye vnc".to_string())
            .map_err(|_| Error::SendVncServerShutdownSignal {})?;
        vnc_server_thread
            .join()
            .map_err(|_| Error::StopVncServerThread {})??;
    }

    log::info!("Successfully shut down");

    Ok(())
}
