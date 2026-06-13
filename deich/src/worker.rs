//! The worker role: runs a Pixelflut server into a time-tracking framebuffer and spawns the
//! background task that syncs that framebuffer to the collector.
//!
//! The collector owns the canvas geometry and frame rate, so the worker fetches its config from
//! the collector before it can allocate the framebuffer and start serving.

use std::{fs, io, path::Path, sync::Arc, time::Duration};

use breakwater::{handle_ctrl_c, server::Server, statistics::StatisticsEvent};
use breakwater_parser::{TimeTrackingFrameBuffer, get_current_ns_since_unix_epoch};
use color_eyre::eyre::{self, Context};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    cli_args::WorkerArgs,
    sync::{self, WorkerConfig},
};

/// Backoff between attempts to reach the collector during startup.
const COLLECTOR_CONNECT_BACKOFF: Duration = Duration::from_secs(1);

/// Runs the worker until Ctrl-C: a Pixelflut server plus the background framebuffer sync.
pub async fn run(args: WorkerArgs) -> eyre::Result<()> {
    let worker_id = load_or_create_worker_id(&args.worker_id_file)?;
    info!(%worker_id, "Starting worker");

    // Fetch the canvas config from the collector (retrying until it's reachable) before we can
    // allocate anything.
    let (stream, config) = connect_to_collector(&args.collector_address, worker_id).await;
    info!(?config, "Received configuration from collector");

    let fb = Arc::new(TimeTrackingFrameBuffer::new(
        config.width as usize,
        config.height as usize,
        get_current_ns_since_unix_epoch(),
    ));
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (terminate_signal_tx, _terminate_signal_rx) = broadcast::channel::<()>(1);

    let network_buffer_size = args
        .network_buffer_size
        .try_into()
        // This should never happen as clap checks the range for us
        .with_context(|| format!("invalid network buffer size: {}", args.network_buffer_size))?;

    let mut server = Server::new(
        &args.listen_addresses,
        fb.clone(),
        statistics_tx.clone(),
        network_buffer_size,
        None,
    )
    .await
    .context("failed to start pixelflut server")?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let stats_drain_thread = tokio::spawn(async move { drain_stats(statistics_rx).await });
    let sync_thread = {
        let fb = fb.clone();
        let collector_address = args.collector_address;
        tokio::spawn(async move {
            sync::sync_framebuffer(fb, collector_address, worker_id, stream, config).await;
        })
    };

    handle_ctrl_c(terminate_signal_tx).await?;
    server_listener_thread.abort();
    stats_drain_thread.abort();
    sync_thread.abort();

    Ok(())
}

/// Connects to the collector, retrying with a backoff until it succeeds.
async fn connect_to_collector(
    collector_address: &str,
    worker_id: Uuid,
) -> (TcpStream, WorkerConfig) {
    loop {
        match sync::connect(collector_address, worker_id).await {
            Ok(result) => return result,
            Err(error) => {
                warn!(
                    collector_address,
                    %error,
                    "Waiting for the collector to become reachable"
                );
                tokio::time::sleep(COLLECTOR_CONNECT_BACKOFF).await;
            }
        }
    }
}

/// Loads this worker's persistent UUID from `path`, generating and saving a fresh one on first run.
fn load_or_create_worker_id(path: &Path) -> eyre::Result<Uuid> {
    match fs::read_to_string(path) {
        Ok(contents) => contents
            .trim()
            .parse()
            .with_context(|| format!("failed to parse worker id from {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let worker_id = Uuid::new_v4();
            fs::write(path, worker_id.to_string())
                .with_context(|| format!("failed to persist worker id to {}", path.display()))?;
            info!(%worker_id, path = %path.display(), "Generated and persisted a new worker id");
            Ok(worker_id)
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to read worker id from {}", path.display()))
        }
    }
}

/// Currently we don't care about stats, so let's just drain them
async fn drain_stats(mut statistics_rx: mpsc::Receiver<StatisticsEvent>) {
    loop {
        if statistics_rx.recv().await.is_none() {
            return;
        }
    }
}
