use std::{env, num::TryFromIntError, sync::Arc};

use breakwater_parser::SimpleFrameBuffer;
use clap::Parser;
use log::info;
use snafu::{ResultExt, Snafu};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinError,
};

use crate::{
    cli_args::CliArgs,
    prometheus_exporter::PrometheusExporter,
    server::Server,
    sinks::{ffmpeg::FfmpegSink, DisplaySink},
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};

mod cli_args;
mod connection_buffer;
mod prometheus_exporter;
mod server;
mod sinks;
mod statistics;
#[cfg(test)]
mod test_helpers;

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

    #[snafu(display("Failed to send termination signal"))]
    SendTerminationSignal {
        source: broadcast::error::SendError<()>,
    },

    #[snafu(display("Failed to create sink"))]
    CreateSink { source: sinks::Error },

    #[snafu(display("Failed to run sink"))]
    RunSink { source: sinks::Error },

    #[snafu(display("Failed to join sink thread"))]
    JoinSinkThread { source: JoinError },

    #[snafu(display("Failed to stop sink"))]
    StopSink { source: sinks::Error },
}

#[tokio::main]
#[snafu::report]
async fn main() -> Result<(), Error> {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let args = CliArgs::parse();

    // Not using dynamic dispatch here for performance reasons
    let fb = Arc::new(SimpleFrameBuffer::new(args.width, args.height));

    // If we make the channel to big, stats will start to lag behind
    // TODO: Check performance impact in real-world scenario. Maybe the statistics thread blocks the other threads
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (statistics_information_tx, statistics_information_rx) =
        broadcast::channel::<StatisticsInformationEvent>(2);
    let (terminate_signal_tx, terminate_signal_rx) = broadcast::channel::<()>(1);

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

    let mut server = Server::new(
        &args.listen_address,
        fb.clone(),
        statistics_tx.clone(),
        args.network_buffer_size
            .try_into()
            // This should never happen as clap checks the range for us
            .context(InvalidNetworkBufferSizeSnafu {
                network_buffer_size: args.network_buffer_size,
            })?,
        args.connections_per_ip,
    )
    .await
    .context(StartPixelflutServerSnafu)?;

    let mut prometheus_exporter = PrometheusExporter::new(
        &args.prometheus_listen_address,
        statistics_information_rx.resubscribe(),
    )
    .context(StartPrometheusExporterSnafu)?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let statistics_thread = tokio::spawn(async move { statistics.run().await });
    let prometheus_exporter_thread = tokio::spawn(async move { prometheus_exporter.run().await });

    let mut display_sinks = Vec::<Box<dyn DisplaySink<SimpleFrameBuffer> + Send>>::new();

    #[cfg(all(feature = "native-display", not(feature = "egui")))]
    {
        use crate::sinks::native_display::NativeDisplaySink;

        if let Some(native_display_sink) = NativeDisplaySink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context(CreateSinkSnafu)?
        {
            display_sinks.push(Box::new(native_display_sink));
        }
    }

    #[cfg(feature = "vnc")]
    {
        use crate::sinks::vnc::VncSink;

        if let Some(vnc_sink) = VncSink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context(CreateSinkSnafu)?
        {
            display_sinks.push(Box::new(vnc_sink));
        }
    }

    let mut ffmpeg_thread_present = false;
    if let Some(ffmpeg_sink) = FfmpegSink::new(
        fb.clone(),
        &args,
        statistics_tx.clone(),
        statistics_information_rx.resubscribe(),
        terminate_signal_rx.resubscribe(),
    )
    .await
    .context(CreateSinkSnafu)?
    {
        display_sinks.push(Box::new(ffmpeg_sink));
        ffmpeg_thread_present = true;
    }

    let mut sink_threads = Vec::new();
    for mut sink in display_sinks {
        sink_threads.push(tokio::spawn(async move {
            sink.run().await?;
            Ok::<(), sinks::Error>(())
        }));
    }

    #[cfg(feature = "egui")]
    {
        use sinks::egui::EguiSink;

        if let Some(mut egui_sink) = EguiSink::new(
            fb.clone(),
            &args,
            statistics_tx.clone(),
            statistics_information_rx.resubscribe(),
            terminate_signal_rx.resubscribe(),
        )
        .await
        .context(CreateSinkSnafu)?
        {
            tokio::spawn(handle_ctrl_c(terminate_signal_tx));

            // Some plattforms require opening windows from the main thread.
            // The tokio::main macro uses Runtime::block_on(future) which runs the future on
            // the current thread, which should be the main thread right now.
            egui_sink.run().await.context(RunSinkSnafu)?;
        } else {
            handle_ctrl_c(terminate_signal_tx).await?;
        }
    }

    #[cfg(not(feature = "egui"))]
    handle_ctrl_c(terminate_signal_tx).await?;

    prometheus_exporter_thread.abort();
    server_listener_thread.abort();

    for sink_thread in sink_threads {
        sink_thread
            .await
            .context(JoinSinkThreadSnafu)?
            .context(StopSinkSnafu)?;
    }

    // We need to stop this thread as the last, as others always try to send statistics to it
    statistics_thread.abort();

    if ffmpeg_thread_present {
        info!("Successfully shut down (there might still be a ffmpeg process running - it's complicated)");
    } else {
        info!("Successfully shut down");
    }

    Ok(())
}

async fn handle_ctrl_c(terminate_signal_tx: broadcast::Sender<()>) -> Result<(), Error> {
    tokio::signal::ctrl_c()
        .await
        .context(WaitForCtrlCSignalSnafu)?;

    terminate_signal_tx
        .send(())
        .context(SendTerminationSignalSnafu)?;

    Ok(())
}
