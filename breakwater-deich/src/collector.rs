//! The collector role: identifies each connecting worker (via its [`Hello`]), tracks the connected
//! set, and stores the frames they stream.
//!
//! A **master** task ticks on the shared [`FrameSchedule`]. Each tick it publishes the window of
//! frame numbers it currently wants — `[render_frame ..= current]`, where `render_frame` is the
//! oldest frame that has had the full streaming budget to arrive — into the shared [`FrameStore`].
//! Worker connections check that window per incoming frame: in-window frames are stored, others are
//! warned about and discarded. The master then merges `render_frame`'s stored frames into a
//! long-term [`Canvas`] by exact (full-`u64`-timestamp) last-write-per-pixel.
//!
//! [`Hello`]: crate::sync::WorkerMessage::Hello

use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    ops::RangeInclusive,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use breakwater::{
    sinks::start_sinks,
    statistics::{
        STATS_REPORT_INTERVAL, StatisticsEvent, StatisticsInformationEvent, StatisticsSaveMode,
    },
};
use breakwater_parser::{
    FB_BYTES_PER_PIXEL, FrameBuffer, MultiPixelSet, SimpleFrameBuffer, TimeTrackingPixel,
    get_current_ns_since_unix_epoch, pixels_as_bytes_mut,
};
use color_eyre::eyre::{self, Context};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc},
    time::interval,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    cli_args::CollectorCliArgs,
    sync::{self, FrameSchedule, WorkerConfig, WorkerMessage},
};

/// How long we assume a worker needs to stream a frame to us (on top of the slot the worker spends
/// finishing it). Together with that slot, this sets how far behind "now" the master renders, so a
/// frame has had time to arrive — see the `margin` computation in [`run_master`].
const FRAME_STREAM_BUDGET: Duration = Duration::from_millis(50);

/// The workers currently connected, keyed by their UUID (value: the connection's peer address, for
/// logging). A duplicate UUID is refused at connect time, so each entry maps to exactly one live
/// connection.
///
/// Reads happen every master tick while writes happen only on (dis)connect, hence a read-optimized
/// `RwLock`. It's a `std` (sync) lock on purpose: the critical sections are a single map op and
/// never cross an `.await`, so an async `tokio::sync::RwLock` would only add overhead.
type ConnectedWorkers = Arc<RwLock<HashMap<Uuid, SocketAddr>>>;

/// Collector-wide statistics, shared between the worker-connection tasks (which feed it) and
/// [`publish_aggregated_statistics`] (which reads it). A `std` (sync) `Mutex` like [`FrameStore`]'s
/// frame map: the critical sections are plain map ops and never cross an `.await`.
type SharedStatistics = Arc<Mutex<CollectorStatistics>>;

/// Holds both the live, per-worker view and the persistent grand totals.
///
/// Each worker reports a snapshot whose `bytes_for_ip` / `denied_connections_for_ip` are cumulative
/// *within its current collector-connection session* — monotonic until the connection drops, then
/// reset to zero on reconnect (the worker rebuilds its aggregator per session). So we turn each
/// worker's counter into deltas and fold them into monotonic grand totals that survive both worker
/// and collector restarts. The per-worker baseline is cleared on disconnect (see [`Self::forget`]),
/// so a reconnecting worker's first snapshot counts in full from zero.
#[derive(Default)]
struct CollectorStatistics {
    /// The latest snapshot from each connected worker, keyed by UUID. Used for the live connection
    /// gauge and as the per-worker baseline for delta accumulation. Removed on disconnect.
    latest_per_worker: HashMap<Uuid, StatisticsInformationEvent>,

    /// Monotonic grand totals, accumulated from per-worker deltas. Persisted to the save file and
    /// seeded from it on startup, so the "big numbers" outlive any restart.
    total_bytes_for_ip: HashMap<IpAddr, u64>,
    total_denied_for_ip: HashMap<IpAddr, u32>,
}

impl CollectorStatistics {
    /// Seeds the grand totals from a previously saved snapshot (the live per-worker view always
    /// starts empty — it's rebuilt as workers (re)connect).
    fn from_save_point(save_point: StatisticsInformationEvent) -> Self {
        Self {
            total_bytes_for_ip: save_point.bytes_for_ip,
            total_denied_for_ip: save_point.denied_connections_for_ip,
            ..Default::default()
        }
    }

    /// Records a worker's fresh snapshot: folds the per-IP increase since its previous snapshot
    /// (zero if this is its first) into the grand totals, then stores it as the new baseline.
    fn record(&mut self, worker_id: Uuid, event: StatisticsInformationEvent) {
        let previous = self.latest_per_worker.get(&worker_id);

        for (&ip, &bytes) in &event.bytes_for_ip {
            let baseline = previous
                .and_then(|p| p.bytes_for_ip.get(&ip))
                .copied()
                .unwrap_or(0);
            // Monotonic within a session, so `bytes >= baseline`; `saturating_sub` only guards the
            // (shouldn't-happen) case of a counter going backwards without a disconnect in between.
            *self.total_bytes_for_ip.entry(ip).or_default() += bytes.saturating_sub(baseline);
        }
        for (&ip, &denied) in &event.denied_connections_for_ip {
            let baseline = previous
                .and_then(|p| p.denied_connections_for_ip.get(&ip))
                .copied()
                .unwrap_or(0);
            let total = self.total_denied_for_ip.entry(ip).or_default();
            *total = total.saturating_add(denied.saturating_sub(baseline));
        }

        self.latest_per_worker.insert(worker_id, event);
    }

    /// Drops a disconnected worker's baseline so its next session accumulates from zero again. Its
    /// already-folded bytes stay in the grand totals.
    fn forget(&mut self, worker_id: Uuid) {
        self.latest_per_worker.remove(&worker_id);
    }

    /// Builds the event published to the sinks: persistent grand totals for bytes/denied, plus the
    /// live connection gauge summed across currently-connected workers. `previous_bytes` carries
    /// the last tick's total so we can derive a per-second rate at the collector.
    fn published_event(&self, previous_bytes: &mut u64) -> StatisticsInformationEvent {
        let mut connections_for_ip: HashMap<IpAddr, u32> = HashMap::new();
        let mut statistic_events = 0;
        for snapshot in self.latest_per_worker.values() {
            for (&ip, &connections) in &snapshot.connections_for_ip {
                *connections_for_ip.entry(ip).or_default() += connections;
            }
            statistic_events += snapshot.statistic_events;
        }

        let connections = connections_for_ip.values().sum();
        let [ips_v6, ips_v4] = connections_for_ip
            .keys()
            .fold([0u32, 0u32], |[v6, v4], ip| match ip {
                IpAddr::V6(_) => [v6 + 1, v4],
                IpAddr::V4(_) => [v6, v4 + 1],
            });

        let bytes: u64 = self.total_bytes_for_ip.values().sum();
        // Rate over one report interval, saturating since a worker dropping out can't shrink the
        // (monotonic) total, but a freshly seeded total on startup can jump the first `previous`.
        let elapsed_secs = STATS_REPORT_INTERVAL.as_secs().max(1);
        let bytes_per_s = bytes.saturating_sub(*previous_bytes) / elapsed_secs;
        *previous_bytes = bytes;

        StatisticsInformationEvent {
            connections,
            ips_v6,
            ips_v4,
            bytes,
            bytes_per_s,
            connections_for_ip,
            denied_connections_for_ip: self.total_denied_for_ip.clone(),
            bytes_for_ip: self.total_bytes_for_ip.clone(),
            statistic_events,
            // Workers don't render, so there's no frame/fps to report at the collector.
            frame: 0,
            fps: 0,
        }
    }

    /// Builds the event written to the save file: only the persistent grand totals matter (the live
    /// view is rebuilt from reconnecting workers), so the other fields stay at their defaults.
    fn save_point(&self) -> StatisticsInformationEvent {
        StatisticsInformationEvent {
            bytes: self.total_bytes_for_ip.values().sum(),
            bytes_for_ip: self.total_bytes_for_ip.clone(),
            denied_connections_for_ip: self.total_denied_for_ip.clone(),
            ..Default::default()
        }
    }
}

/// The long-term merged canvas, held by the master for the whole process lifetime.
///
/// Each pixel keeps its full write timestamp (the high bits of [`TimeTrackingPixel`]), so
/// last-write-wins is exact over arbitrary time and survives traffic gaps and worker restarts.
/// It's a flat pixel vector — no width/height; merging is purely per-index.
struct Canvas {
    pixels: Vec<TimeTrackingPixel>,
    /// Reused scratch for [`Self::draw_to_framebuffer`]'s RGB byte layout, so the per-tick draw
    /// doesn't allocate a fresh multi-megabyte buffer at the frame rate.
    rgb_scratch: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        let pixel_count = width * height;
        Self {
            pixels: vec![TimeTrackingPixel::default(); pixel_count],
            rgb_scratch: Vec::with_capacity(pixel_count * FB_BYTES_PER_PIXEL),
        }
    }

    /// Folds `frame` in, keeping for each pixel whichever write has the larger timestamp. Since the
    /// default timestamp is `0` (oldest possible), a never-written pixel — a blank or restarted
    /// worker's frame — never clobbers fresher content.
    fn merge(&mut self, frame: &[TimeTrackingPixel]) {
        for (canvas_pixel, &frame_pixel) in self.pixels.iter_mut().zip(frame) {
            if frame_pixel.timestamp() > canvas_pixel.timestamp() {
                *canvas_pixel = frame_pixel;
            }
        }
    }

    fn draw_to_framebuffer<FB: FrameBuffer + MultiPixelSet>(&mut self, fb: &Arc<FB>) {
        self.rgb_scratch.clear();
        self.rgb_scratch
            .extend(self.pixels.iter().flat_map(|pixel| pixel.rgb().to_le_bytes()));
        fb.set_multi_from_start_index(0, &self.rgb_scratch);
    }
}

/// How an incoming frame relates to the window of frames the master currently wants.
enum FrameInterest {
    /// In the window — store it.
    Wanted,
    /// Outside the window. Carries the window so the caller can tell whether the frame is too old
    /// (arrived late) or from the future, and by how much.
    OutsideWindow { window: RangeInclusive<u64> },
    /// The master hasn't published a window yet (just started up).
    NoWindowYet,
}

/// Shared between the master task and the worker-connection tasks.
struct FrameStore {
    /// Frame numbers the master currently wants: the inclusive window `[render_frame ..= current]`.
    /// Read by every worker connection per frame; written by the master once per tick. `None`
    /// until the master's first tick.
    interesting: RwLock<Option<RangeInclusive<u64>>>,

    /// Stored frames: `frame_number -> (worker_id -> framebuffer)`. Written by workers, read and
    /// evicted by the master.
    frames: Mutex<HashMap<u64, HashMap<Uuid, Vec<TimeTrackingPixel>>>>,
}

impl FrameStore {
    fn new() -> Self {
        Self {
            interesting: RwLock::new(None),
            frames: Mutex::new(HashMap::new()),
        }
    }

    /// Classifies an incoming frame against the window the master currently wants.
    fn classify(&self, frame_number: u64) -> FrameInterest {
        match self
            .interesting
            .read()
            .expect("interesting-window lock poisoned")
            .as_ref()
        {
            None => FrameInterest::NoWindowYet,
            Some(window) if window.contains(&frame_number) => FrameInterest::Wanted,
            Some(window) => FrameInterest::OutsideWindow {
                window: window.clone(),
            },
        }
    }

    /// Stores a worker's frame (overwriting any previous frame from the same worker for that slot).
    fn store(&self, frame_number: u64, worker_id: Uuid, frame: Vec<TimeTrackingPixel>) {
        self.frames
            .lock()
            .expect("frame store lock poisoned")
            .entry(frame_number)
            .or_default()
            .insert(worker_id, frame);
    }

    /// Publishes the new interesting `window`, evicts frames below it (they missed their render),
    /// and removes & returns `render_frame`'s frames so the caller can merge them without holding
    /// the lock.
    fn advance(
        &self,
        window: RangeInclusive<u64>,
        render_frame: u64,
    ) -> HashMap<Uuid, Vec<TimeTrackingPixel>> {
        *self
            .interesting
            .write()
            .expect("interesting-window lock poisoned") = Some(window);

        let mut frames = self.frames.lock().expect("frame store lock poisoned");
        frames.retain(|&frame_number, _| frame_number >= render_frame);
        frames.remove(&render_frame).unwrap_or_default()
    }
}

/// Runs the collector until Ctrl-C: accepts worker connections, configures them, stores frames, and
/// runs the master render-scheduling task.
pub async fn run(args: CollectorCliArgs) -> eyre::Result<()> {
    let config = WorkerConfig {
        width: args.width,
        height: args.height,
        sync_fps: args.fps,
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

    let connected_workers: ConnectedWorkers = Arc::new(RwLock::new(HashMap::new()));
    let frame_store = Arc::new(FrameStore::new());

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

    // Most of the time we want to render the screen to *something*, otherwise the game is boring
    // As the breakwater sinks expect the memory layout of the [`SimpleFrameBuffer`] we need to
    // create and maintain one.
    let render_fb = Arc::new(SimpleFrameBuffer::new(
        args.width as usize,
        args.height as usize,
    ));

    // If we make the channel to big, stats will start to lag behind
    // TODO: Check performance impact in real-world scenario. Maybe the statistics thread blocks the other threads
    let (statistics_tx, mut statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    // The aggregated, per-IP statistics we publish for the sinks to render. Fed by
    // `publish_aggregated_statistics` below from the snapshots workers stream in.
    let (statistics_information_tx, statistics_information_rx) =
        broadcast::channel::<StatisticsInformationEvent>(2);

    // The collector itself produces no raw statistics events; only sinks (e.g. the VNC sink's
    // rendered-frame counter) might. We don't surface those yet, so drain them.
    let stats_task = tokio::spawn(async move { while statistics_rx.recv().await.is_some() {} });

    {
        let frame_store = frame_store.clone();
        let render_fb = render_fb.clone();
        let connected_workers = connected_workers.clone();
        tokio::spawn(async move {
            run_master(&frame_store, &connected_workers, config, render_fb).await;
        });
    }

    tokio::spawn(publish_aggregated_statistics(
        statistics.clone(),
        statistics_information_tx,
        statistics_save_mode,
    ));

    let accept_task = tokio::spawn(accept_workers(
        listener,
        connected_workers,
        statistics,
        frame_store,
        config,
    ));

    let (sink_tasks, ffmpeg_thread_present) = start_sinks(
        &args.sinks,
        render_fb.clone(),
        // There are no listen addresses we know about (the traffic comes in via a complicated
        // path - virtual IP and stuff)
        &[],
        args.fps,
        statistics_tx,
        statistics_information_rx,
    )
    .await
    .context("failed to start sinks")?;

    accept_task.abort();

    for sink_task in sink_tasks {
        sink_task
            .await
            .context("failed to join sink task")?
            .context("failed to stop sink")?;
    }

    // We need to stop this task last, as others (the sinks) always try to send statistics to it
    stats_task.abort();

    if ffmpeg_thread_present {
        tracing::info!(
            "successfully shut down (there might still be a ffmpeg process running - it's complicated)"
        );
    } else {
        tracing::info!("successfully shut down");
    }

    Ok(())
}

/// Accepts worker connections forever, spawning a handler per connection. Only returns if accepting
/// itself fails.
async fn accept_workers(
    listener: TcpListener,
    connected_workers: ConnectedWorkers,
    statistics: SharedStatistics,
    frame_store: Arc<FrameStore>,
    config: WorkerConfig,
) -> eyre::Result<()> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept worker connection")?;

        let connected_workers = connected_workers.clone();
        let statistics = statistics.clone();
        let frame_store = frame_store.clone();
        tokio::spawn(async move {
            match handle_worker(
                stream,
                peer,
                config,
                &connected_workers,
                &statistics,
                &frame_store,
            )
            .await
            {
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

/// The master render-scheduling task. Ticks on the shared schedule; each tick it takes the workers'
/// frames for the frame it is rendering and merges them (latest write per pixel) into the long-term
/// canvas.
///
/// The canvas is held here for the whole process lifetime, so it survives traffic gaps and worker
/// restarts: with no new frames it simply keeps its last contents, and a restarted worker's blank
/// frame can't overwrite live pixels (a blank pixel is never "newer" than real content).
async fn run_master(
    frame_store: &FrameStore,
    connected_workers: &ConnectedWorkers,
    config: WorkerConfig,
    render_fb: Arc<SimpleFrameBuffer>,
) {
    let schedule = FrameSchedule::new(config.sync_fps);
    // How many slots behind "now" the master renders. A worker only sends frame N once slot N has
    // *ended* — one period after it began — so a frame is already a full slot old before streaming
    // even starts. The `1 +` covers that slot-completion delay; `frames_spanning(..)` adds the
    // streaming budget on top. (Without the `1 +`, `--fps 1` rendered a slot whose frame was still
    // in flight, so nothing ever merged.)
    let margin = 1 + schedule.frames_spanning(FRAME_STREAM_BUDGET);

    let mut canvas = Canvas::new(config.width as usize, config.height as usize);

    let mut current_frame = schedule.frame_number_at(get_current_ns_since_unix_epoch());
    loop {
        // Tick at the start of `current_frame`'s slot.
        let now = get_current_ns_since_unix_epoch();
        let slot_start = schedule.frame_start_ns(current_frame);
        tokio::time::sleep(Duration::from_nanos(slot_start.saturating_sub(now))).await;

        let render_frame = current_frame.saturating_sub(margin);
        let frames = frame_store.advance(render_frame..=current_frame, render_frame);
        let connected = connected_workers
            .read()
            .expect("connected workers lock poisoned")
            .len();

        // Fold each worker's frame for this slot into the canvas (exact latest-write-per-pixel).
        for frame in frames.values() {
            canvas.merge(frame);
        }

        canvas.draw_to_framebuffer(&render_fb);

        debug!(
            render_frame,
            frames_merged = frames.len(),
            connected_workers = connected,
            "Master tick: merged frames into the canvas"
        );

        // Advance to the current slot, skipping any we fell behind on, always moving forward.
        current_frame = schedule
            .frame_number_at(get_current_ns_since_unix_epoch())
            .max(current_frame + 1);
    }
}

/// Reads the worker's hello, registers it, then stores its frames until the connection drops,
/// finally deregistering it.
async fn handle_worker(
    mut stream: TcpStream,
    peer: SocketAddr,
    config: WorkerConfig,
    connected_workers: &ConnectedWorkers,
    statistics: &SharedStatistics,
    frame_store: &FrameStore,
) -> io::Result<()> {
    let worker_id = sync::accept_worker(&mut stream).await?;

    if !register(connected_workers, worker_id, peer) {
        warn!(
            %worker_id,
            %peer,
            "Worker id is already connected; refusing this duplicate connection"
        );
        // Returning drops `stream`, which closes (slams) the connection.
        return Ok(());
    }

    let result = serve_worker(&mut stream, peer, worker_id, config, statistics, frame_store).await;

    deregister(connected_workers, worker_id, peer);
    // Drop this worker's baseline so its next session accumulates from zero; its already-folded
    // grand totals stay.
    statistics
        .lock()
        .expect("collector statistics lock poisoned")
        .forget(worker_id);
    result
}

/// Sends the worker its config, then reads its messages forever: storing the frames the master
/// wants and recording each statistics snapshot, until the connection drops.
async fn serve_worker(
    stream: &mut TcpStream,
    peer: SocketAddr,
    worker_id: Uuid,
    config: WorkerConfig,
    statistics: &SharedStatistics,
    frame_store: &FrameStore,
) -> io::Result<()> {
    sync::send_config(stream, config).await?;

    let schedule = FrameSchedule::new(config.sync_fps);
    let pixel_count = config.width as usize * config.height as usize;
    // Reused only for discarding frames the master doesn't want.
    let mut discard = vec![0u8; config.frame_size_bytes()];

    loop {
        match sync::receive_worker_message(stream).await? {
            WorkerMessage::Frame { frame_number } => match frame_store.classify(frame_number) {
                FrameInterest::Wanted => {
                    // Read straight into a fresh pixel buffer (outside any lock) so we can hand
                    // ownership to the store; `read_exact` writes directly into its bytes, no copy.
                    let mut buffer = vec![TimeTrackingPixel::default(); pixel_count];
                    sync::receive_frame_body(stream, pixels_as_bytes_mut(&mut buffer)).await?;
                    frame_store.store(frame_number, worker_id, buffer);
                }
                other => {
                    // Still consume the blob so the stream stays aligned for the next message.
                    sync::receive_frame_body(stream, &mut discard).await?;
                    log_dropped_frame(peer, frame_number, &other, schedule);
                }
            },
            WorkerMessage::Statistics(event) => {
                // Fold this snapshot's increase into the grand totals and store it as the new
                // baseline; the publisher reads the result on its next tick.
                statistics
                    .lock()
                    .expect("collector statistics lock poisoned")
                    .record(worker_id, event);
            }
            // The hello is a one-shot handshake consumed in `accept_worker`; a second one is a
            // protocol violation.
            WorkerMessage::Hello { worker_id: duplicate } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected second hello (worker id {duplicate}) mid-stream"),
                ));
            }
        }
    }
}

/// Logs a frame the master didn't want, explaining whether it was outdated or from the future and
/// by how much, alongside the tolerance (the size of the window the master accepts).
fn log_dropped_frame(
    peer: SocketAddr,
    frame_number: u64,
    interest: &FrameInterest,
    schedule: FrameSchedule,
) {
    match interest {
        // Can't reach here for `Wanted`, but keep the match exhaustive and cheap.
        FrameInterest::Wanted => {}
        FrameInterest::NoWindowYet => {
            debug!(%peer, frame_number, "Dropping frame: the master hasn't started rendering yet");
        }
        FrameInterest::OutsideWindow { window } => {
            // The window is `[oldest ..= newest]`; `newest` is the master's current slot and the
            // span is how far back it still accepts frames.
            let tolerance = schedule.duration_of_frames(window.end() - window.start());
            if frame_number < *window.start() {
                // Older than the oldest slot we still accept: it arrived too late.
                let delayed_by = schedule.duration_of_frames(window.end() - frame_number);
                warn!(
                    %peer,
                    frame_number,
                    ?delayed_by,
                    ?tolerance,
                    "Dropping outdated frame: it arrived later than the collector tolerates"
                );
            } else {
                // Newer than the master's current slot: the worker's clock is ahead of ours.
                let ahead_by = schedule.duration_of_frames(frame_number - window.end());
                warn!(
                    %peer,
                    frame_number,
                    ?ahead_by,
                    ?tolerance,
                    "Dropping frame from the future (is the worker's clock ahead of the collector's?)"
                );
            }
        }
    }
}

/// Registers a worker. Returns `false` (without inserting) if its UUID is already connected.
fn register(connected_workers: &ConnectedWorkers, worker_id: Uuid, peer: SocketAddr) -> bool {
    let mut workers = connected_workers
        .write()
        .expect("connected workers lock poisoned");
    if workers.contains_key(&worker_id) {
        return false;
    }

    workers.insert(worker_id, peer);
    info!(
        %worker_id,
        %peer,
        connected_workers = workers.len(),
        "Worker connected"
    );
    true
}

fn deregister(connected_workers: &ConnectedWorkers, worker_id: Uuid, peer: SocketAddr) {
    let mut workers = connected_workers
        .write()
        .expect("connected workers lock poisoned");
    // Duplicates are refused at connect time, so the entry is guaranteed to be ours to remove.
    workers.remove(&worker_id);
    info!(
        %worker_id,
        %peer,
        connected_workers = workers.len(),
        "Worker disconnected"
    );
}

/// Runs for the whole process lifetime, doing two things on their own intervals:
/// - every [`STATS_REPORT_INTERVAL`], publishes the aggregated event on the broadcast channel the
///   sinks already consume — so the overlay renders the merged, per-IP view with no extra work;
/// - every configured save interval (when enabled), persists the grand totals to the save file so
///   the "big numbers" survive a collector restart.
async fn publish_aggregated_statistics(
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a frame from `(rgb, timestamp)` pairs.
    fn frame(pixels: &[(u32, u64)]) -> Vec<TimeTrackingPixel> {
        pixels
            .iter()
            .map(|&(rgb, timestamp)| TimeTrackingPixel::new(rgb, timestamp))
            .collect()
    }

    /// Builds a worker snapshot carrying per-IP byte and (live) connection counts.
    fn snapshot(
        bytes_for_ip: &[(IpAddr, u64)],
        connections_for_ip: &[(IpAddr, u32)],
    ) -> StatisticsInformationEvent {
        StatisticsInformationEvent {
            bytes_for_ip: bytes_for_ip.iter().copied().collect(),
            connections_for_ip: connections_for_ip.iter().copied().collect(),
            statistic_events: 1,
            ..Default::default()
        }
    }

    #[test]
    fn accumulates_per_ip_totals_and_live_connections_across_workers() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        let (w1, w2) = (Uuid::from_u128(1), Uuid::from_u128(2));

        // `v4` hit both workers; `v6` only the second. Totals and the shared IP must add up.
        let mut stats = CollectorStatistics::default();
        stats.record(w1, snapshot(&[(v4, 100)], &[(v4, 2)]));
        stats.record(w2, snapshot(&[(v4, 50), (v6, 7)], &[(v4, 1), (v6, 3)]));

        let mut previous_bytes = 0;
        let event = stats.published_event(&mut previous_bytes);

        // Bytes are the persistent grand total.
        assert_eq!(event.bytes_for_ip[&v4], 150);
        assert_eq!(event.bytes_for_ip[&v6], 7);
        assert_eq!(event.bytes, 157);
        // Connections are the live gauge, summed across currently-connected workers.
        assert_eq!(event.connections_for_ip[&v4], 3);
        assert_eq!(event.connections, 6);
        assert_eq!(event.ips_v4, 1);
        assert_eq!(event.ips_v6, 1);
        assert_eq!(event.statistic_events, 2);
        // First tick: the whole total counts as this interval's throughput.
        assert_eq!(event.bytes_per_s, 157 / STATS_REPORT_INTERVAL.as_secs().max(1));
        assert_eq!(previous_bytes, 157);
    }

    #[test]
    fn folds_deltas_so_a_cumulative_counter_is_not_double_counted() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();

        // A worker's counter is cumulative within a session: only the increase between consecutive
        // snapshots is folded into the grand total.
        stats.record(w, snapshot(&[(v4, 100)], &[]));
        stats.record(w, snapshot(&[(v4, 175)], &[]));
        assert_eq!(stats.total_bytes_for_ip[&v4], 175);
    }

    #[test]
    fn worker_restart_keeps_totals_without_dipping_or_double_counting() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();

        stats.record(w, snapshot(&[(v4, 100)], &[(v4, 1)]));
        // Worker disconnects: its baseline is dropped, but its folded bytes stay in the total.
        stats.forget(w);
        assert_eq!(stats.total_bytes_for_ip[&v4], 100);

        // It reconnects (same UUID) with a counter reset to zero. The first post-restart snapshot
        // counts in full, so the total grows by exactly the new traffic — no dip, no double count.
        stats.record(w, snapshot(&[(v4, 30)], &[(v4, 1)]));
        assert_eq!(stats.total_bytes_for_ip[&v4], 130);
    }

    #[test]
    fn accumulates_denied_connections() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let denied = |n: u32| StatisticsInformationEvent {
            denied_connections_for_ip: [(v4, n)].into_iter().collect(),
            ..Default::default()
        };

        let mut stats = CollectorStatistics::default();
        stats.record(w, denied(3));
        stats.record(w, denied(5)); // cumulative -> grand total +2
        assert_eq!(stats.total_denied_for_ip[&v4], 5);
        assert_eq!(stats.published_event(&mut 0).denied_connections_for_ip[&v4], 5);
    }

    #[test]
    fn save_point_seeds_grand_totals_on_restart() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();
        stats.record(w, snapshot(&[(v4, 4096)], &[]));

        // Collector restart: persist, then seed a fresh instance from the save point.
        let reseeded = CollectorStatistics::from_save_point(stats.save_point());
        assert_eq!(reseeded.total_bytes_for_ip[&v4], 4096);
        // The live view starts empty; workers reconnect from zero and accumulate on top of the seed.
        assert!(reseeded.latest_per_worker.is_empty());
    }

    #[test]
    fn classify_distinguishes_wanted_old_and_new() {
        let store = FrameStore::new();

        // Before the master ticks, there's no window yet.
        assert!(matches!(store.classify(42), FrameInterest::NoWindowYet));

        *store.interesting.write().unwrap() = Some(40..=42);

        assert!(matches!(store.classify(41), FrameInterest::Wanted));
        // Older than the window -> outdated; newer than the window -> from the future.
        assert!(matches!(
            store.classify(38),
            FrameInterest::OutsideWindow { .. }
        ));
        assert!(matches!(
            store.classify(50),
            FrameInterest::OutsideWindow { .. }
        ));
    }

    #[test]
    fn merges_latest_timestamp_per_pixel() {
        let mut canvas = Canvas::new(1, 2);

        // Pixel 0: A timestamp 1320, B timestamp 5032 -> B wins.
        // Pixel 1: A timestamp 4200, B timestamp 0 (never written) -> A stays.
        canvas.merge(&frame(&[(0xaa_0000, 1_320), (0xaa_0001, 4_200)]));
        canvas.merge(&frame(&[(0x00_00bb, 5_032), (0x00_00bc, 0)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x00_00bb);
        assert_eq!(canvas.pixels[0].timestamp(), 5_032);
        assert_eq!(canvas.pixels[1].rgb(), 0xaa_0001);
        assert_eq!(canvas.pixels[1].timestamp(), 4_200);
    }

    #[test]
    fn blank_frame_never_overwrites_live_content() {
        let mut canvas = Canvas::new(1, 1);
        canvas.merge(&frame(&[(0x12_3456, 50)]));

        // A never-written pixel has timestamp 0 (oldest possible), so it can't clobber live content
        // (the restarted-worker / blank-canvas case).
        canvas.merge(&frame(&[(0, 0)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x12_3456);
        assert_eq!(canvas.pixels[0].timestamp(), 50);
    }

    #[test]
    fn older_write_does_not_replace_newer() {
        let mut canvas = Canvas::new(1, 1);
        // Merge the newer write first, then an older one — order must not matter.
        canvas.merge(&frame(&[(0x00_00bb, 5_032)]));
        canvas.merge(&frame(&[(0xaa_0000, 1_320)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x00_00bb);
        assert_eq!(canvas.pixels[0].timestamp(), 5_032);
    }
}
