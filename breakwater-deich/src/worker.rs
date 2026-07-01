//! The worker role: runs a Pixelflut server into a time-tracking framebuffer and syncs that
//! framebuffer to the collector.
//!
//! The collector owns the canvas geometry, frame rate and timestamp epoch, so the worker fetches
//! its config from the collector before it can allocate the framebuffer and start serving. Each
//! [`run_session`] runs until the collector connection drops; the worker then tears everything down
//! and starts a fresh session, which transparently picks up a changed config (geometry, fps, or a
//! new epoch after a collector restart).

use std::{fs, io, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use breakwater::{
    server::Server,
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};
use breakwater_parser::TimeTrackingFrameBuffer;
use color_eyre::eyre::{self, Context};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    cli_args::WorkerCliArgs,
    sync::{self, WorkerConfig},
};

/// Backoff between worker sessions (also used while waiting for the collector to become reachable).
const SESSION_BACKOFF: Duration = Duration::from_secs(1);

/// Runs the worker until Ctrl-C, restarting the session whenever the collector connection drops.
pub async fn run(args: WorkerCliArgs) -> eyre::Result<()> {
    let worker_id = load_or_create_worker_id(&args.worker_id_file)?;
    info!(%worker_id, "Starting worker");

    tokio::select! {
        // `session_loop` never returns on its own — it just keeps (re)starting sessions.
        result = session_loop(&args, worker_id) => result,
        result = tokio::signal::ctrl_c() => {
            result.context("failed to wait for CTRL + C")?;
            info!("Received CTRL + C, shutting down");
            Ok(())
        }
    }
}

/// Runs sessions back-to-back forever; a session ending (connection lost, config change, error) is
/// just logged and followed by a fresh one after a short backoff.
async fn session_loop(args: &WorkerCliArgs, worker_id: Uuid) -> eyre::Result<()> {
    loop {
        match run_session(args, worker_id).await {
            Ok(()) => info!("Collector connection ended; restarting worker session"),
            Err(error) => warn!(%error, "Worker session failed; restarting"),
        }
        tokio::time::sleep(SESSION_BACKOFF).await;
    }
}

/// One worker session: connect to the collector, build the framebuffer from its config, serve
/// Pixelflut into it, and sync it — until the collector connection drops (or the server stops).
///
/// The server, stats drain and sync all run as `select!` arms (not detached tasks), so when the
/// session ends — here, or because the whole worker is cancelled on Ctrl-C — they're all dropped
/// together. No teardown bookkeeping, no leaked tasks.
async fn run_session(args: &WorkerCliArgs, worker_id: Uuid) -> eyre::Result<()> {
    let (mut stream, config) = connect_to_collector(args.collector_address, worker_id).await;
    info!(?config, "Received configuration from collector");

    let fb = Arc::new(TimeTrackingFrameBuffer::new(
        config.width as usize,
        config.height as usize,
        config.epoch_ns_since_unix_epoch,
    ));
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);

    // Worker-local aggregator: it folds the server's raw per-connection events into a periodic,
    // per-IP snapshot (~once per second) that we forward to the collector, which in turn merges the
    // snapshots across all workers. Persisting/merging across workers is the collector's job, so the
    // worker runs the aggregator with saving disabled.
    let (statistics_information_tx, _) = broadcast::channel::<StatisticsInformationEvent>(2);
    let mut statistics = Statistics::new(
        statistics_rx,
        statistics_information_tx.clone(),
        StatisticsSaveMode::Disabled,
    )
    .context("failed to create statistics aggregator")?;

    let network_buffer_size = args
        .network_listener
        .network_buffer_size
        .try_into()
        // This should never happen as clap checks the range for us
        .with_context(|| {
            format!(
                "invalid network buffer size: {}",
                args.network_listener.network_buffer_size
            )
        })?;

    let mut server = Server::new(
        &args.network_listener.listen_addresses,
        fb.clone(),
        statistics_tx,
        network_buffer_size,
        args.network_listener.connections_per_ip,
    )
    .await
    .context("failed to start pixelflut server")?;

    tokio::select! {
        result = server.start() => result.context("pixelflut server stopped")?,
        result = statistics.run() => result.context("statistics aggregator stopped")?,
        result = sync::sync(&fb, &mut stream, config, statistics_information_tx.subscribe()) => {
            result.context("framebuffer and statistics sync to the collector stopped")?;
        }
    }

    Ok(())
}

/// Connects to the collector, retrying with a backoff until it succeeds.
async fn connect_to_collector(
    collector_address: SocketAddr,
    worker_id: Uuid,
) -> (TcpStream, WorkerConfig) {
    loop {
        match sync::connect(collector_address, worker_id).await {
            Ok(result) => return result,
            Err(error) => {
                warn!(
                    %collector_address,
                    %error,
                    "Waiting for the collector to become reachable"
                );
                tokio::time::sleep(SESSION_BACKOFF).await;
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
