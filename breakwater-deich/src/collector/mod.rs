//! The collector role: accepts worker connections, keeps each worker's latest framebuffer, and on a
//! fixed render cadence merges them into a long-term [`Canvas`] that the sinks display.
//!
//! There is no frame numbering, window or shared schedule. Because the canvas merges by absolute
//! per-pixel timestamp (last-write-wins, which is *commutative*) and each worker sends a *full*
//! framebuffer that supersedes its earlier ones, the collector can simply keep only the newest frame
//! per worker and fold them all in every render tick — order and arrival timing don't matter. See
//! [`Canvas`] for the correctness argument.

mod canvas;
mod stats;

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use breakwater::{
    sinks::start_sinks,
    statistics::{
        STATS_REPORT_INTERVAL, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode,
    },
};
use breakwater_parser::{SimpleFrameBuffer, TimeTrackingPixel, get_current_ns_since_unix_epoch};
use color_eyre::eyre::{self, Context};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc},
    time::{MissedTickBehavior, interval},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    cli_args::CollectorCliArgs,
    collector::{canvas::Canvas, stats::CollectorStatistics},
    metrics::{self, CollectorMetrics, METRICS_LOG_INTERVAL, StatisticsMetrics},
    protocol::{self, Connection, WorkerConfig, WorkerData, frame_period},
};

/// Each connected worker's latest full framebuffer (`None` until its first frame arrives).
type LatestFrame = Option<Arc<Vec<TimeTrackingPixel>>>;

/// The connected workers, keyed by UUID. A duplicate UUID is refused at connect time, so each entry
/// maps to exactly one live connection. Read by the render loop every tick and written by the worker
/// connections on (dis)connect and per frame — a `std` (sync) `Mutex` because every critical section
/// is a short map op that never crosses an `.await`.
type Workers = Arc<Mutex<HashMap<Uuid, LatestFrame>>>;

/// Collector-wide statistics, shared between the worker-connection tasks (which feed it) and
/// [`publish_statistics`] (which reads it).
type SharedStatistics = Arc<Mutex<CollectorStatistics>>;

/// Runs the collector until Ctrl-C (or a sink error): accepts worker connections, stores their
/// latest frames, renders the merged canvas, and publishes aggregated statistics.
pub async fn run(args: CollectorCliArgs) -> eyre::Result<()> {
    let config = WorkerConfig {
        width: args.width,
        height: args.height,
        fps: args.fps,
        // Our startup time: the zero point for every worker's per-pixel timestamps.
        epoch_ns_since_unix_epoch: get_current_ns_since_unix_epoch(),
    };

    let listener = TcpListener::bind(args.listen_address)
        .await
        .with_context(|| format!("failed to bind collector to {}", args.listen_address))?;
    info!(
        listen_address = %args.listen_address,
        ?config,
        frame_size = config.frame_size_bytes(),
        "Collector listening for workers"
    );

    let workers: Workers = Arc::new(Mutex::new(HashMap::new()));

    // Persistent per-IP statistics: seed the grand totals from the save file (if any) so the "big
    // numbers" survive restarts; the live per-worker view is rebuilt as workers (re)connect.
    let statistics_save_mode = StatisticsSaveMode::from(args.statistics_save_file);
    let statistics = match &statistics_save_mode {
        StatisticsSaveMode::Enabled { save_file, .. } => {
            match StatisticsInformationEvent::load_from_file(save_file)? {
                Some(save_point) => {
                    info!(%save_file, "Restored statistics from save file");
                    CollectorStatistics::from_save_point(save_point)
                }
                None => CollectorStatistics::default(),
            }
        }
        StatisticsSaveMode::Disabled => CollectorStatistics::default(),
    };
    let statistics: SharedStatistics = Arc::new(Mutex::new(statistics));

    let metrics =
        Arc::new(CollectorMetrics::new().context("failed to register collector metrics")?);
    metrics::serve(args.prometheus.prometheus_listen_address)?;

    // The breakwater sinks expect the memory layout of a [`SimpleFrameBuffer`], so the render loop
    // draws the merged canvas into one for them to consume.
    let render_fb = Arc::new(SimpleFrameBuffer::new(
        args.width as usize,
        args.height as usize,
    ));

    // If we make the channel too big, stats will start to lag behind.
    let (statistics_tx, mut statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (statistics_information_tx, statistics_information_rx) =
        broadcast::channel::<StatisticsInformationEvent>(2);

    // Expose the aggregated per-IP statistics as Prometheus metrics, using the same metric names as
    // breakwater. Subscribe before the sender is moved into `publish_statistics` below.
    let mut statistics_metrics = StatisticsMetrics::new(statistics_information_tx.subscribe())
        .context("failed to register statistics metrics")?;

    // Support tasks: every handle is kept (no detached tasks) so shutdown is deterministic. The
    // collector produces no raw statistics events itself; only sinks (e.g. the VNC sink's rendered
    // -frame counter) might, so we drain them. Sinks can still emit during their own teardown, so
    // the drain is aborted *last*, after the sink tasks are joined.
    let drain_task = tokio::spawn(async move { while statistics_rx.recv().await.is_some() {} });
    let accept_task = tokio::spawn(accept_workers(
        listener,
        workers.clone(),
        statistics.clone(),
        metrics.clone(),
        config,
    ));
    let render_task = tokio::spawn(render_loop(workers.clone(), render_fb.clone(), config));
    let publish_task = tokio::spawn(publish_statistics(
        statistics.clone(),
        statistics_information_tx,
        statistics_save_mode,
    ));
    let metrics_task = tokio::spawn(log_metrics(metrics.clone(), workers.clone()));
    let statistics_metrics_task = tokio::spawn(async move { statistics_metrics.run().await });

    let (sink_tasks, ffmpeg_thread_present) = start_sinks(
        &args.sinks,
        render_fb.clone(),
        // There are no listen addresses we know about — traffic reaches the workers via a virtual
        // IP and stuff, not the collector.
        &[],
        config.fps,
        statistics_tx,
        statistics_information_rx,
    )
    .await
    .context("failed to start sinks")?;

    // `start_sinks` returns once shutdown is triggered (a sink erred, or Ctrl-C). Stop the support
    // tasks, join the sinks, then stop the stats drain last (see above).
    accept_task.abort();
    render_task.abort();
    publish_task.abort();
    metrics_task.abort();
    statistics_metrics_task.abort();

    for sink_task in sink_tasks {
        sink_task
            .await
            .context("failed to join sink task")?
            .context("failed to stop sink")?;
    }

    drain_task.abort();

    if ffmpeg_thread_present {
        info!(
            "successfully shut down (there might still be a ffmpeg process running - it's complicated)"
        );
    } else {
        info!("successfully shut down");
    }

    Ok(())
}

/// Accepts worker connections forever, spawning a handler per connection. Only returns if accepting
/// itself fails.
async fn accept_workers(
    listener: TcpListener,
    workers: Workers,
    statistics: SharedStatistics,
    metrics: Arc<CollectorMetrics>,
    config: WorkerConfig,
) -> eyre::Result<()> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept worker connection")?;

        let workers = workers.clone();
        let statistics = statistics.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            match handle_worker(stream, peer, config, &workers, &statistics, &metrics).await {
                // The read loop only ever returns on error; a clean disconnect shows up as EOF.
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    debug!(%peer, "Worker connection closed");
                }
                Err(error) => warn!(%peer, %error, "Worker connection error"),
                Ok(()) => {}
            }
        });
    }
}

/// Reads the worker's hello, registers it, serves it until the connection drops, then deregisters
/// it (and drops its statistics baseline).
async fn handle_worker(
    stream: TcpStream,
    peer: SocketAddr,
    config: WorkerConfig,
    workers: &Workers,
    statistics: &SharedStatistics,
    metrics: &CollectorMetrics,
) -> io::Result<()> {
    let (mut connection, worker_id) = protocol::accept(stream).await?;

    if !register(workers, worker_id, peer) {
        warn!(
            %worker_id,
            %peer,
            "Worker id is already connected; refusing this duplicate connection"
        );
        // Returning drops `connection`, which closes it.
        return Ok(());
    }

    let result = serve_worker(
        &mut connection,
        worker_id,
        config,
        workers,
        statistics,
        metrics,
    )
    .await;

    deregister(workers, worker_id, peer);
    // Drop this worker's baseline so its next session accumulates from zero; its already-folded
    // grand totals stay.
    statistics
        .lock()
        .expect("collector statistics lock poisoned")
        .forget(worker_id);
    result
}

/// Sends the worker its config, then reads its messages forever: keeping its latest framebuffer and
/// recording each statistics snapshot, until the connection drops.
async fn serve_worker(
    connection: &mut Connection<TcpStream>,
    worker_id: Uuid,
    config: WorkerConfig,
    workers: &Workers,
    statistics: &SharedStatistics,
    metrics: &CollectorMetrics,
) -> io::Result<()> {
    connection.send_config(config).await?;
    let pixel_count = config.pixel_count();
    let frame_size_bytes = config.frame_size_bytes() as u64;
    // Cache this worker's metric handles once, so the per-frame path skips the label lookup.
    let recorder = metrics.recorder(worker_id);

    loop {
        match connection.recv_worker_data(pixel_count).await? {
            WorkerData::Framebuffer {
                pixels,
                recv_duration,
                latency,
            } => {
                recorder.observe_frame(recv_duration, latency, frame_size_bytes);
                // Keep only this worker's latest frame; the render loop merges it. A full frame
                // supersedes the worker's earlier ones, so there is nothing to accumulate here.
                if let Some(slot) = workers
                    .lock()
                    .expect("workers lock poisoned")
                    .get_mut(&worker_id)
                {
                    *slot = Some(Arc::new(pixels));
                }
            }
            WorkerData::Statistics(event) => {
                // Fold this snapshot's increase into the grand totals; the publisher reads the
                // result on its next tick.
                statistics
                    .lock()
                    .expect("collector statistics lock poisoned")
                    .record(worker_id, event);
            }
        }
    }
}

/// Renders at the configured frame rate: folds every connected worker's latest framebuffer into the
/// long-term canvas (exact latest-write-per-pixel) and draws it to the framebuffer the sinks read.
///
/// The canvas is persistent for the whole process lifetime, so it survives traffic gaps and worker
/// restarts: with no new frames it keeps its last contents, and a restarted worker's blank frame
/// can't overwrite live pixels (a blank pixel is never "newer" than real content).
async fn render_loop(workers: Workers, render_fb: Arc<SimpleFrameBuffer>, config: WorkerConfig) {
    let mut canvas = Canvas::new(config.width as usize, config.height as usize);
    let mut ticker = interval(frame_period(config.fps));
    // Under load, pace renders a period apart rather than bursting to catch up on missed ticks.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        // Snapshot the current per-worker frames (cheap `Arc` clones) under a short lock, then merge
        // outside it so worker connections never block on rendering.
        let frames: Vec<Arc<Vec<TimeTrackingPixel>>> = workers
            .lock()
            .expect("workers lock poisoned")
            .values()
            .filter_map(Clone::clone)
            .collect();

        for frame in &frames {
            canvas.merge(frame);
        }
        canvas.draw_to_framebuffer(&render_fb);
    }
}

/// Logs a per-interval summary of collector ingress (bytes/s and the implied link utilisation) plus
/// the live worker count. Per-worker receive time and end-to-end latency live in the Prometheus
/// histograms; this is just the at-a-glance "is the collector link keeping up" line. Never returns.
async fn log_metrics(metrics: Arc<CollectorMetrics>, workers: Workers) {
    let mut ticker = interval(METRICS_LOG_INTERVAL);
    let interval_secs = METRICS_LOG_INTERVAL.as_secs().max(1);
    let mut previous_bytes = 0u64;

    loop {
        ticker.tick().await;

        let bytes = metrics.ingress_bytes_total();
        let delta = bytes.saturating_sub(previous_bytes);
        previous_bytes = bytes;
        let mb_per_s = delta as f64 / interval_secs as f64 / 1_000_000.0;
        let connected = workers.lock().expect("workers lock poisoned").len();

        info!(
            connected_workers = connected,
            ingress_mb_s = format!("{mb_per_s:.1}"),
            ingress_gbit_s = format!("{:.2}", mb_per_s * 8.0 / 1000.0),
            "Collector ingress metrics (last interval)"
        );
    }
}

/// Runs for the whole process lifetime, doing two things on their own intervals:
/// - every [`STATS_REPORT_INTERVAL`], publishes the aggregated event on the broadcast channel the
///   sinks already consume — so the overlay renders the merged, per-IP view with no extra work;
/// - every configured save interval (when enabled), persists the grand totals to the save file so
///   the "big numbers" survive a collector restart.
async fn publish_statistics(
    statistics: SharedStatistics,
    statistics_information_tx: broadcast::Sender<StatisticsInformationEvent>,
    save_mode: StatisticsSaveMode,
) {
    let mut report = interval(STATS_REPORT_INTERVAL);
    // Mirror breakwater's `Statistics`: when saving is disabled, an effectively-never timer keeps
    // the `select!` arm valid without firing.
    let (mut save, save_file) = match &save_mode {
        StatisticsSaveMode::Disabled => (interval(Duration::MAX), None),
        StatisticsSaveMode::Enabled {
            save_file,
            interval: save_interval,
        } => (interval(*save_interval), Some(save_file.clone())),
    };

    // Previous tick's total byte count, so we can derive a per-second rate at the collector.
    let mut previous_bytes = 0u64;

    loop {
        tokio::select! {
            _ = report.tick() => {
                let event = statistics
                    .lock()
                    .expect("collector statistics lock poisoned")
                    .published_event(&mut previous_bytes);
                // A send error just means no sink is currently subscribed; nothing to do about it.
                let _ = statistics_information_tx.send(event);
            }
            _ = save.tick() => {
                if let Some(save_file) = &save_file {
                    let save_point = statistics
                        .lock()
                        .expect("collector statistics lock poisoned")
                        .save_point();
                    if let Err(error) = save_point.save_to_file(save_file) {
                        warn!(%save_file, %error, "Failed to save statistics");
                    }
                }
            }
        }
    }
}

/// Registers a worker (with no frame yet). Returns `false` (without inserting) if its UUID is
/// already connected.
fn register(workers: &Workers, worker_id: Uuid, peer: SocketAddr) -> bool {
    let mut workers = workers.lock().expect("workers lock poisoned");
    if workers.contains_key(&worker_id) {
        return false;
    }

    workers.insert(worker_id, None);
    info!(
        %worker_id,
        %peer,
        connected_workers = workers.len(),
        "Worker connected"
    );
    true
}

fn deregister(workers: &Workers, worker_id: Uuid, peer: SocketAddr) {
    let mut workers = workers.lock().expect("workers lock poisoned");
    // Duplicates are refused at connect time, so the entry is guaranteed to be ours to remove.
    workers.remove(&worker_id);
    info!(
        %worker_id,
        %peer,
        connected_workers = workers.len(),
        "Worker disconnected"
    );
}
