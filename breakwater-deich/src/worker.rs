//! The worker role: runs a Pixelflut server into a time-tracking framebuffer and streams that
//! framebuffer to the collector.
//!
//! The collector owns the canvas geometry, frame rate and timestamp epoch, so the worker fetches its
//! config from the collector before it can allocate the framebuffer and start serving. Each
//! `run_session` runs until the collector connection drops; the worker then tears everything down
//! and starts a fresh session, which transparently picks up a changed config (geometry, fps, or a
//! new epoch after a collector restart).

use std::{
    fs, io,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use breakwater::{
    server::Server,
    statistics::{Statistics, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode},
};
use breakwater_parser::{TimeTrackingFrameBuffer, get_current_ns_since_unix_epoch};
use color_eyre::eyre::{self, Context};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
    time::{MissedTickBehavior, interval},
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    cli_args::WorkerCliArgs,
    metrics::{self, METRICS_LOG_INTERVAL, WorkerMetrics},
    protocol::{self, Connection, WorkerConfig, frame_period},
};

/// Backoff between worker sessions (also used while waiting for the collector to become reachable).
const SESSION_BACKOFF: Duration = Duration::from_secs(1);

/// Runs the worker until Ctrl-C, restarting the session whenever the collector connection drops.
pub async fn run(args: WorkerCliArgs) -> eyre::Result<()> {
    let worker_id = load_or_create_worker_id(&args.worker_id_file)?;
    info!(%worker_id, "Starting worker");

    // Created once for the whole process so the metrics accumulate across sessions.
    let metrics = WorkerMetrics::new().context("failed to register worker metrics")?;
    metrics::serve(args.prometheus.prometheus_listen_address)?;

    tokio::select! {
        // `session_loop` never returns on its own — it just keeps (re)starting sessions.
        result = session_loop(&args, worker_id, &metrics) => result,
        // Neither does the metrics logger; it just runs alongside.
        () = log_metrics(&metrics) => unreachable!("metrics log loop never returns"),
        result = tokio::signal::ctrl_c() => {
            result.context("failed to wait for CTRL + C")?;
            info!("Received CTRL + C, shutting down");
            Ok(())
        }
    }
}

/// Logs a per-interval summary of the worker's send metrics (mean push time and frames lagged),
/// diffing the cumulative Prometheus counters between ticks. Never returns.
async fn log_metrics(metrics: &WorkerMetrics) {
    let mut ticker = interval(METRICS_LOG_INTERVAL);
    let (mut prev_count, mut prev_sum, mut prev_lagged) = (0u64, 0.0, 0u64);

    loop {
        ticker.tick().await;
        let (count, sum) = metrics.send_totals();
        let lagged = metrics.lagged_total();
        let frames = count - prev_count;
        let mean_ms = if frames > 0 {
            (sum - prev_sum) / frames as f64 * 1000.0
        } else {
            0.0
        };
        info!(
            frames,
            mean_send_ms = format!("{mean_ms:.1}"),
            lagged = lagged - prev_lagged,
            "Worker send metrics (last interval)"
        );
        (prev_count, prev_sum, prev_lagged) = (count, sum, lagged);
    }
}

/// Runs sessions back-to-back forever; a session ending (connection lost, config change, error) is
/// just logged and followed by a fresh one after a short backoff.
async fn session_loop(
    args: &WorkerCliArgs,
    worker_id: Uuid,
    metrics: &WorkerMetrics,
) -> eyre::Result<()> {
    loop {
        match run_session(args, worker_id, metrics).await {
            Ok(()) => info!("Collector connection ended; restarting worker session"),
            Err(error) => warn!(%error, "Worker session failed; restarting"),
        }
        tokio::time::sleep(SESSION_BACKOFF).await;
    }
}

/// One worker session: connect to the collector, build the framebuffer from its config, serve
/// Pixelflut into it, and stream it — until the collector connection drops (or the server stops).
///
/// The server, stats aggregator and push loop all run as `select!` arms (not detached tasks), so
/// when the session ends — here, or because the whole worker is cancelled on Ctrl-C — they're all
/// dropped together. No teardown bookkeeping, no leaked tasks.
async fn run_session(
    args: &WorkerCliArgs,
    worker_id: Uuid,
    metrics: &WorkerMetrics,
) -> eyre::Result<()> {
    let (mut connection, config) = connect_to_collector(args.collector_address, worker_id).await;
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
        result = push_frames(&mut connection, &fb, config.fps, statistics_information_tx.subscribe(), metrics) => {
            result.context("framebuffer and statistics sync to the collector stopped")?;
        }
    }

    Ok(())
}

/// Streams the framebuffer and periodic statistics to the collector over the one connection, until
/// it fails. Frames go out on a local timer at the configured rate; per-pixel timestamps are
/// absolute (set by the framebuffer at write time), so nothing needs re-basing and the collector
/// resolves ordering itself. Statistics snapshots (~once per second) are forwarded as they arrive.
///
/// Both message kinds share this one task, so their writes can never interleave — a statistics
/// message can't slip between a framebuffer's marker and its raw blob.
async fn push_frames(
    connection: &mut Connection<TcpStream>,
    fb: &TimeTrackingFrameBuffer,
    fps: u32,
    mut statistics_rx: broadcast::Receiver<StatisticsInformationEvent>,
    metrics: &WorkerMetrics,
) -> io::Result<()> {
    let period = frame_period(fps);
    let mut ticker = interval(period);
    // If a push runs long (slow network), don't burst to catch up — just space the next one a full
    // period out. The collector only cares about the latest frame anyway.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let sent_at_ns = get_current_ns_since_unix_epoch();
                let started = Instant::now();
                connection.send_framebuffer(sent_at_ns, fb.as_raw_bytes()).await?;
                let elapsed = started.elapsed();

                metrics.observe_send(elapsed);
                // A send spanning N whole frame-periods means the N-1 slots after the first went
                // unsent — i.e. we couldn't keep up with the target FPS.
                let spanned = (elapsed.as_nanos() / period.as_nanos().max(1)) as u64;
                metrics.add_lagged(spanned.saturating_sub(1));
            }
            event = statistics_rx.recv() => match event {
                Ok(event) => connection.send_statistics(event).await?,
                // Lagged: snapshots are emitted ~once per second into a small buffer with a single
                // consumer, so lagging shouldn't happen; if it somehow does, skip the dropped ones.
                // Closed: the aggregator is gone, but the whole session is torn down at that point
                // (it runs as a sibling `select!` arm), so this is effectively unreachable. Either
                // way: keep streaming frames rather than failing the sync.
                Err(broadcast::error::RecvError::Lagged(_) | broadcast::error::RecvError::Closed) => {}
            },
        }
    }
}

/// Connects to the collector, retrying with a backoff until it succeeds.
async fn connect_to_collector(
    collector_address: SocketAddr,
    worker_id: Uuid,
) -> (Connection<TcpStream>, WorkerConfig) {
    loop {
        match protocol::connect(collector_address, worker_id).await {
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
