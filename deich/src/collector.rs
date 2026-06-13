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
    net::SocketAddr,
    ops::RangeInclusive,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use breakwater_parser::{TimeTrackingPixel, get_current_ns_since_unix_epoch, pixels_as_bytes_mut};
use color_eyre::eyre::{self, Context};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    cli_args::CollectorArgs,
    sync::{self, FrameSchedule, WorkerConfig},
};

/// How long we assume a worker needs to stream a frame to us. The master keeps the last
/// `⌈budget / frame_period⌉` frames "interesting" and renders the oldest of them, so it only
/// renders a frame once every worker has had this long to deliver it.
const FRAME_STREAM_BUDGET: Duration = Duration::from_millis(50);

/// The workers currently connected, keyed by their UUID (value: the connection's peer address, for
/// logging). A duplicate UUID is refused at connect time, so each entry maps to exactly one live
/// connection.
///
/// Reads happen every master tick while writes happen only on (dis)connect, hence a read-optimized
/// `RwLock`. It's a `std` (sync) lock on purpose: the critical sections are a single map op and
/// never cross an `.await`, so an async `tokio::sync::RwLock` would only add overhead.
type ConnectedWorkers = Arc<RwLock<HashMap<Uuid, SocketAddr>>>;

/// The long-term merged canvas, held by the master for the whole process lifetime.
///
/// Each pixel keeps its full write timestamp (the high bits of [`TimeTrackingPixel`]), so
/// last-write-wins is exact over arbitrary time and survives traffic gaps and worker restarts.
/// It's a flat pixel vector — no width/height; merging is purely per-index.
struct Canvas {
    pixels: Vec<TimeTrackingPixel>,
}

impl Canvas {
    fn new(pixel_count: usize) -> Self {
        Self {
            pixels: vec![TimeTrackingPixel::default(); pixel_count],
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

    /// Whether the master currently wants frame `frame_number`.
    fn is_interested(&self, frame_number: u64) -> bool {
        self.interesting
            .read()
            .expect("interesting-window lock poisoned")
            .as_ref()
            .is_some_and(|window| window.contains(&frame_number))
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
pub async fn run(args: CollectorArgs) -> eyre::Result<()> {
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

    {
        let frame_store = frame_store.clone();
        let connected_workers = connected_workers.clone();
        tokio::spawn(async move { run_master(&frame_store, &connected_workers, config).await });
    }

    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .context("failed to accept worker connection")?;

        let connected_workers = connected_workers.clone();
        let frame_store = frame_store.clone();
        tokio::spawn(async move {
            match handle_worker(stream, peer, config, &connected_workers, &frame_store).await {
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
) {
    let schedule = FrameSchedule::new(config.sync_fps);
    // Render the oldest frame that has had `FRAME_STREAM_BUDGET` to arrive.
    let margin = schedule.frames_spanning(FRAME_STREAM_BUDGET);

    let mut canvas = Canvas::new(config.width as usize * config.height as usize);

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

        // TODO: render `canvas` to the screen / output sink.
        info!(
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

    let result = serve_frames(&mut stream, peer, worker_id, config, frame_store).await;

    deregister(connected_workers, worker_id, peer);
    result
}

/// Sends the worker its config, then reads its frames forever, storing the ones the master wants.
async fn serve_frames(
    stream: &mut TcpStream,
    peer: SocketAddr,
    worker_id: Uuid,
    config: WorkerConfig,
    frame_store: &FrameStore,
) -> io::Result<()> {
    sync::send_config(stream, config).await?;

    let pixel_count = config.width as usize * config.height as usize;
    // Reused only for discarding frames the master doesn't want.
    let mut discard = vec![0u8; config.frame_size_bytes()];

    loop {
        let frame_number = sync::receive_frame_number(stream).await?;

        if frame_store.is_interested(frame_number) {
            // Read straight into a fresh pixel buffer (outside any lock) so we can hand ownership to
            // the store; `read_exact` writes directly into its bytes, no extra copy.
            let mut buffer = vec![TimeTrackingPixel::default(); pixel_count];
            sync::receive_frame_body(stream, pixels_as_bytes_mut(&mut buffer)).await?;
            frame_store.store(frame_number, worker_id, buffer);
        } else {
            // Still consume the blob so the stream stays aligned for the next message.
            sync::receive_frame_body(stream, &mut discard).await?;
            warn!(
                %peer,
                frame_number,
                "Master is not interested in this frame (outside the window); dropping it"
            );
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

    #[test]
    fn merges_latest_timestamp_per_pixel() {
        let mut canvas = Canvas::new(2);

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
        let mut canvas = Canvas::new(1);
        canvas.merge(&frame(&[(0x12_3456, 50)]));

        // A never-written pixel has timestamp 0 (oldest possible), so it can't clobber live content
        // (the restarted-worker / blank-canvas case).
        canvas.merge(&frame(&[(0, 0)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x12_3456);
        assert_eq!(canvas.pixels[0].timestamp(), 50);
    }

    #[test]
    fn older_write_does_not_replace_newer() {
        let mut canvas = Canvas::new(1);
        // Merge the newer write first, then an older one — order must not matter.
        canvas.merge(&frame(&[(0x00_00bb, 5_032)]));
        canvas.merge(&frame(&[(0xaa_0000, 1_320)]));

        assert_eq!(canvas.pixels[0].rgb(), 0x00_00bb);
        assert_eq!(canvas.pixels[0].timestamp(), 5_032);
    }
}
