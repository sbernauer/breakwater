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

/// One worker's framebuffer for one frame, plus the timestamp base it's relative to. Stored as
/// [`TimeTrackingPixel`]s (rather than raw bytes) to keep the layout tied to the canonical type;
/// the wire bytes are read straight into this buffer via a `bytemuck` cast.
struct ReceivedFrame {
    /// The base the pixels' `coarse_ns_since_base` values are relative to.
    base_ns_since_unix_epoch: u64,
    buffer: Vec<TimeTrackingPixel>,
}

/// The long-term merged canvas, held by the master for the whole process lifetime.
///
/// Unlike a worker's framebuffer (whose 3-byte `coarse_ns` only spans ~536 ms), each pixel here
/// keeps a full `u64` absolute write timestamp, so last-write-wins is *exact* over arbitrary time
/// and survives traffic gaps and worker restarts. It's a flat pixel vector — no width/height,
/// merging is purely per-index.
struct Canvas {
    pixels: Vec<CanvasPixel>,
}

#[derive(Clone, Copy, Default)]
struct CanvasPixel {
    /// The winning color. Written by the merge; read by the (future) renderer and the tests.
    // Not read in the binary yet (no rendering); `expect` can't be used as the tests *do* read it.
    #[allow(dead_code)]
    rgb: [u8; 3],
    /// Absolute UNIX-epoch time the winning write happened; `0` means "never written".
    written_ns_since_unix_epoch: u64,
}

impl Canvas {
    fn new(pixel_count: usize) -> Self {
        Self {
            pixels: vec![CanvasPixel::default(); pixel_count],
        }
    }

    /// Folds `frame` in, keeping for each pixel the write with the latest absolute timestamp.
    /// Pixels the frame carries no recent-write info for (`coarse_ns == 0`) are skipped, so a blank
    /// or stale frame never clobbers fresher content.
    fn merge(&mut self, frame: &ReceivedFrame) {
        for (canvas_pixel, frame_pixel) in self.pixels.iter_mut().zip(&frame.buffer) {
            if let Some(written_ns) =
                frame_pixel.written_ns_since_unix_epoch(frame.base_ns_since_unix_epoch)
                && written_ns > canvas_pixel.written_ns_since_unix_epoch
            {
                canvas_pixel.rgb = frame_pixel.rgb;
                canvas_pixel.written_ns_since_unix_epoch = written_ns;
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

    /// Stored frames: `frame_number -> (worker_id -> frame)`. Written by workers, read and evicted
    /// by the master.
    frames: Mutex<HashMap<u64, HashMap<Uuid, ReceivedFrame>>>,
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
    fn store(&self, frame_number: u64, worker_id: Uuid, frame: ReceivedFrame) {
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
    ) -> HashMap<Uuid, ReceivedFrame> {
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
        let header = sync::receive_frame_header(stream).await?;

        if frame_store.is_interested(header.frame_number) {
            // Read straight into a fresh pixel buffer (outside any lock) so we can hand ownership to
            // the store; `read_exact` writes directly into its bytes, no extra copy.
            let mut buffer = vec![TimeTrackingPixel::default(); pixel_count];
            sync::receive_frame_body(stream, pixels_as_bytes_mut(&mut buffer)).await?;
            frame_store.store(
                header.frame_number,
                worker_id,
                ReceivedFrame {
                    base_ns_since_unix_epoch: header.base_ns_since_unix_epoch,
                    buffer,
                },
            );
        } else {
            // Still consume the blob so the stream stays aligned for the next message.
            sync::receive_frame_body(stream, &mut discard).await?;
            warn!(
                %peer,
                frame_number = header.frame_number,
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

    /// Builds a `ReceivedFrame` from `(rgb, coarse_ns)` pairs.
    fn frame(base_ns_since_unix_epoch: u64, pixels: &[([u8; 3], u32)]) -> ReceivedFrame {
        let buffer = pixels
            .iter()
            .map(|&(rgb, coarse)| {
                let coarse = coarse.to_le_bytes();
                TimeTrackingPixel {
                    rgb,
                    coarse_ns_since_base: [coarse[0], coarse[1], coarse[2]],
                }
            })
            .collect();
        ReceivedFrame {
            base_ns_since_unix_epoch,
            buffer,
        }
    }

    #[test]
    fn merges_latest_absolute_write_per_pixel() {
        let mut canvas = Canvas::new(2);

        // Pixel 0: A at 1000 + (10 << 5) = 1320; B at 5000 + (1 << 5) = 5032 -> B wins.
        // Pixel 1: A at 1000 + (100 << 5) = 4200; B has coarse 0 -> skipped -> A stays.
        let a = frame(1_000, &[([0xaa, 0, 0], 10), ([0xaa, 0, 1], 100)]);
        let b = frame(5_000, &[([0, 0, 0xbb], 1), ([0, 0, 0xbc], 0)]);

        canvas.merge(&a);
        canvas.merge(&b);

        assert_eq!(canvas.pixels[0].rgb, [0, 0, 0xbb]);
        assert_eq!(canvas.pixels[0].written_ns_since_unix_epoch, 5_032);
        assert_eq!(canvas.pixels[1].rgb, [0xaa, 0, 1]);
        assert_eq!(canvas.pixels[1].written_ns_since_unix_epoch, 4_200);
    }

    #[test]
    fn blank_or_stale_frame_never_overwrites_live_content() {
        let mut canvas = Canvas::new(1);
        canvas.merge(&frame(1_000, &[([0x12, 0x34, 0x56], 50)]));

        // A frame with coarse 0 carries no recent-write info — even with a far-later base it must
        // not clobber the live pixel (this is the restarted-worker / blank-canvas case).
        canvas.merge(&frame(9_000_000_000, &[([0, 0, 0], 0)]));

        assert_eq!(canvas.pixels[0].rgb, [0x12, 0x34, 0x56]);
        assert_eq!(
            canvas.pixels[0].written_ns_since_unix_epoch,
            1_000 + (50 << 5)
        );
    }

    #[test]
    fn older_write_does_not_replace_newer() {
        let mut canvas = Canvas::new(1);
        // Merge the newer write first, then an older one — order must not matter.
        canvas.merge(&frame(5_000, &[([0, 0, 0xbb], 1)])); // 5032
        canvas.merge(&frame(1_000, &[([0xaa, 0, 0], 10)])); // 1320, older

        assert_eq!(canvas.pixels[0].rgb, [0, 0, 0xbb]);
        assert_eq!(canvas.pixels[0].written_ns_since_unix_epoch, 5_032);
    }
}
