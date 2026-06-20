use std::sync::Arc;

use breakwater_parser::SharedMemoryFrameBuffer;
use clap::{CommandFactory, FromArgMatches};
use color_eyre::eyre::{self, Context};
use server::Server;
use tokio::sync::{broadcast, mpsc};

use crate::{
    cli_args::CliArgs,
    sinks::start_sinks,
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};

mod cli_args;
mod connection_buffer;
#[cfg(feature = "prometheus")]
mod prometheus_exporter;
mod server;
mod sinks;
mod statistics;
#[cfg(test)]
mod test_helpers;

#[cfg(test)]
mod tests;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(if cfg!(debug_assertions) {
            tracing::Level::DEBUG.into()
        } else {
            tracing::Level::INFO.into()
        })
        .from_env()?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // We parse via `ArgMatches` (instead of `CliArgs::parse()`) so that `SinkCliArgs::validate` can use
    // `value_source` to tell which sink options were actually passed on the command line.
    let mut cmd = CliArgs::command();
    let matches = cmd.get_matches_mut();
    let args = CliArgs::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if let Err(e) = args.sinks.validate(&mut cmd, &matches) {
        e.exit();
    }

    // Not using dynamic dispatch here for performance reasons
    let fb = Arc::new(
        SharedMemoryFrameBuffer::new(args.width, args.height, args.shared_memory_name.as_deref())
            .context("failed to create shared memory framebuffer")?,
    );

    // If we make the channel to big, stats will start to lag behind
    // TODO: Check performance impact in real-world scenario. Maybe the statistics thread blocks the other threads
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (statistics_information_tx, statistics_information_rx) =
        broadcast::channel::<StatisticsInformationEvent>(2);

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
    )?;

    let mut server = Server::new(
        &args.network_listener.listen_addresses,
        fb.clone(),
        statistics_tx.clone(),
        args.network_listener
            .network_buffer_size
            .try_into()
            // This should never happen as clap checks the range for us
            .with_context(|| {
                format!(
                    "invalid network buffer size: {}",
                    args.network_listener.network_buffer_size
                )
            })?,
        args.network_listener.connections_per_ip,
    )
    .await
    .context("failed to start pixelflut server")?;

    #[cfg(feature = "prometheus")]
    let mut prometheus_exporter = prometheus_exporter::PrometheusExporter::new(
        &args.prometheus_listen_address,
        statistics_information_rx.resubscribe(),
    )
    .context("failed to start prometheus exporter")?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let statistics_thread = tokio::spawn(async move { statistics.run().await });
    #[cfg(feature = "prometheus")]
    let prometheus_exporter_thread = tokio::spawn(async move { prometheus_exporter.run().await });

    let (sink_tasks, ffmpeg_thread_present) = start_sinks(
        &args.sinks,
        fb.clone(),
        &args.network_listener.listen_addresses,
        args.fps,
        statistics_tx,
        statistics_information_rx,
    )
    .await
    .context("failed to start sinks")?;

    #[cfg(feature = "prometheus")]
    prometheus_exporter_thread.abort();
    server_listener_thread.abort();

    for sink_task in sink_tasks {
        sink_task
            .await
            .context("failed to join sink task")?
            .context("failed to stop sink")?;
    }

    // We need to stop this thread last, as others always try to send statistics to it
    statistics_thread.abort();

    if ffmpeg_thread_present {
        tracing::info!(
            "successfully shut down (there might still be a ffmpeg process running - it's complicated)"
        );
    } else {
        tracing::info!("successfully shut down");
    }

    Ok(())
}
