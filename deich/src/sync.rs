//! Worker ↔ collector sync protocol.
//!
//! Flow: a worker connects to the collector and announces itself with a handshake (magic) plus a
//! [`WorkerMessage::Hello`] carrying its persistent UUID, so the collector knows which worker the
//! connection belongs to. The collector replies with a [`CollectorMessage::Config`] — it owns
//! canvas geometry and frame rate — and the worker then streams frames until the connection drops.
//!
//! Control messages are serialized with [`postcard`] and length-delimited (a little-endian `u32`
//! length prefix), so the message set can grow into richer enums over time. The 12 MB framebuffer
//! blob is deliberately *not* serialized: a [`WorkerMessage::Frame`] header is immediately followed
//! on the wire by `frame_size` raw bytes, keeping serialization off the hot payload.

use std::{io, sync::Arc, time::Duration};

use breakwater_parser::{
    FrameBuffer, TimeTrackingFrameBuffer, TimeTrackingPixel, get_current_ns_since_unix_epoch,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tracing::{info, warn};
use uuid::Uuid;

/// Identifies the deich sync protocol on the wire ("deic" in ASCII). The collector and workers are
/// always deployed together, so a single magic to reject obviously-wrong connections is enough; we
/// don't bother with version negotiation.
const MAGIC: u32 = 0x6465_6963;

/// Upper bound on a (length-delimited) control message, to reject garbage before allocating.
/// Control messages are tiny; the big framebuffer blob is sent raw, outside this path.
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// How long a worker waits between attempts to (re)connect to the collector.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

/// Messages the collector sends to a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectorMessage {
    /// Reply to the worker's [`WorkerMessage::Hello`]; tells the worker how to configure itself.
    Config(WorkerConfig),
}

/// Messages a worker sends to the collector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerMessage {
    /// First message after the handshake: identifies which worker this connection belongs to.
    Hello { worker_id: Uuid },

    /// A framebuffer frame. On the wire this header is immediately followed by
    /// [`WorkerConfig::frame_size`] raw framebuffer bytes (not part of this serialized message).
    Frame(FrameHeader),
}

/// Metadata sent ahead of each raw framebuffer blob.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FrameHeader {
    /// Which scheduled slot this frame is for (see [`FrameSchedule`]). Because every node derives
    /// it from the same wall-clock schedule, the collector can tell when all workers' frames for a
    /// slot have arrived, and discard frames for slots it has already rendered.
    pub frame_number: u64,

    /// The base the frame's per-pixel `coarse_ns_since_base` values are relative to. This is the
    /// worker's *actual* re-base time, deliberately not `frame_number * period`: a temporarily
    /// overloaded worker may re-base a slot late, and the timestamps must stay correct regardless.
    pub base_ns_since_unix_epoch: u64,
}

/// The wall-clock frame schedule shared by every worker and the collector. Frames are numbered by
/// how many `1/fps`-long slots have elapsed since the UNIX epoch, so each node maps an instant to a
/// slot independently and still agrees on the number — no node is the master clock. (Server clocks
/// here are within ~µs of each other, far below a tens-of-milliseconds slot.)
#[derive(Debug, Clone, Copy)]
pub struct FrameSchedule {
    frame_period_ns: u64,
}

impl FrameSchedule {
    pub fn new(fps: u32) -> Self {
        Self {
            frame_period_ns: (1_000_000_000 / u64::from(fps)).max(1),
        }
    }

    /// The frame slot the given UNIX-epoch instant falls into.
    pub fn frame_number_at(&self, ns_since_unix_epoch: u64) -> u64 {
        ns_since_unix_epoch / self.frame_period_ns
    }

    /// The UNIX-epoch nanosecond timestamp at which `frame_number` begins.
    pub fn frame_start_ns(&self, frame_number: u64) -> u64 {
        frame_number * self.frame_period_ns
    }

    /// How many whole frames `duration` spans, rounded up. Used to size delay margins.
    pub fn frames_spanning(&self, duration: Duration) -> u64 {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        ns.div_ceil(self.frame_period_ns)
    }
}

/// Configuration the collector hands to each worker. The collector is the single source of truth
/// for canvas geometry and frame rate; workers configure themselves from it. Extend this when
/// workers need more, e.g. their offset within a larger canvas.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub width: u32,
    pub height: u32,
    pub sync_fps: u32,
}

impl WorkerConfig {
    /// Number of bytes in one framebuffer blob.
    pub fn frame_size_bytes(&self) -> usize {
        self.width as usize * self.height as usize * size_of::<TimeTrackingPixel>()
    }
}

/// Worker side: connect to the collector, announce ourselves with `worker_id`, and read the config
/// the collector replies with.
pub async fn connect(
    collector_address: &str,
    worker_id: Uuid,
) -> io::Result<(TcpStream, WorkerConfig)> {
    let mut stream = TcpStream::connect(collector_address).await?;
    write_handshake(&mut stream).await?;
    write_message(&mut stream, &WorkerMessage::Hello { worker_id }).await?;

    let CollectorMessage::Config(config) = read_message(&mut stream).await?;
    Ok((stream, config))
}

/// Collector side: validate the handshake and read the worker's [`WorkerMessage::Hello`], returning
/// the worker's UUID so the connection can be attributed to it.
pub async fn accept_worker<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Uuid> {
    read_handshake(reader).await?;
    match read_message(reader).await? {
        WorkerMessage::Hello { worker_id } => Ok(worker_id),
        WorkerMessage::Frame { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a hello message but got a frame",
        )),
    }
}

/// Collector side: send the worker its config in reply to the hello.
pub async fn send_config<W: AsyncWrite + Unpin>(
    writer: &mut W,
    config: WorkerConfig,
) -> io::Result<()> {
    write_message(writer, &CollectorMessage::Config(config)).await
}

/// Collector side: read a frame's [`FrameHeader`]. The caller must then read exactly
/// [`WorkerConfig::frame_size_bytes`] bytes with [`receive_frame_body`] before the next message,
/// or discard them — splitting the two lets the caller decide whether to keep the blob.
pub async fn receive_frame_header<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<FrameHeader> {
    match read_message(reader).await? {
        WorkerMessage::Frame(header) => Ok(header),
        WorkerMessage::Hello { worker_id } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a frame but got another hello from {worker_id}"),
        )),
    }
}

/// Collector side: read a frame's raw blob into `pixels` (which must be
/// [`WorkerConfig::frame_size_bytes`] long), following a [`receive_frame_header`].
pub async fn receive_frame_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    pixels: &mut [u8],
) -> io::Result<()> {
    reader.read_exact(pixels).await?;
    Ok(())
}

/// Worker side: stream the framebuffer to the collector forever, reconnecting as needed.
///
/// `stream`/`config` are the live connection and config from the initial [`connect`]. On every
/// (re)connect the collector re-sends the config; we honour the frame rate, but since the
/// framebuffer is already allocated we only warn if the geometry changed (live resize is not
/// supported yet).
pub async fn sync_framebuffer(
    fb: Arc<TimeTrackingFrameBuffer>,
    collector_address: String,
    worker_id: Uuid,
    mut stream: TcpStream,
    mut config: WorkerConfig,
) {
    loop {
        let schedule = FrameSchedule::new(config.sync_fps);
        if let Err(error) = stream_frames(&fb, &mut stream, schedule).await {
            warn!(%error, "Framebuffer sync stream failed, reconnecting");
        }

        (stream, config) = reconnect(&fb, &collector_address, worker_id).await;
    }
}

/// Streams frames aligned to the shared [`FrameSchedule`] until a write fails (returns the error).
///
/// Each iteration sleeps until the current slot ends, sends the frame accumulated during it, then
/// re-bases the framebuffer for the next slot. The slot the worker enters is recomputed from the
/// clock each time, so a worker that falls behind (e.g. temporarily overloaded) skips the slots it
/// missed instead of sending a backlog of stale frames.
async fn stream_frames(
    fb: &TimeTrackingFrameBuffer,
    stream: &mut TcpStream,
    schedule: FrameSchedule,
) -> io::Result<()> {
    // Start accumulating the current slot, basing pixels at the actual time we started.
    fb.set_base_ns_since_unix_epoch(get_current_ns_since_unix_epoch());
    let mut frame_number = schedule.frame_number_at(get_current_ns_since_unix_epoch());

    loop {
        // Sleep until the slot we're accumulating ends.
        let slot_end = schedule.frame_start_ns(frame_number + 1);
        let now = get_current_ns_since_unix_epoch();
        tokio::time::sleep(Duration::from_nanos(slot_end.saturating_sub(now))).await;

        // Send the just-completed slot's frame. `base_ns` is the framebuffer's *actual* base (set
        // below for this slot), so the collector interprets `coarse_ns` correctly even if we
        // re-based late; `frame_number` is the schedule slot, for the collector's barrier.
        let header = FrameHeader {
            frame_number,
            base_ns_since_unix_epoch: fb.base_ns_since_unix_epoch(),
        };
        write_message(stream, &WorkerMessage::Frame(header)).await?;
        stream.write_all(fb.as_bytes()).await?;
        stream.flush().await?;

        // Re-base for the next slot at the actual current time, and advance to whichever slot that
        // is — skipping any we fell behind on, while always moving forward by at least one.
        let now = get_current_ns_since_unix_epoch();
        fb.set_base_ns_since_unix_epoch(now);
        frame_number = schedule.frame_number_at(now).max(frame_number + 1);
    }
}

/// Reconnects to the collector (retrying with a backoff), returning the new stream and config.
async fn reconnect(
    fb: &TimeTrackingFrameBuffer,
    collector_address: &str,
    worker_id: Uuid,
) -> (TcpStream, WorkerConfig) {
    loop {
        match connect(collector_address, worker_id).await {
            Ok((stream, config)) => {
                if config.width as usize != fb.get_width()
                    || config.height as usize != fb.get_height()
                {
                    warn!(
                        ?config,
                        fb_width = fb.get_width(),
                        fb_height = fb.get_height(),
                        "Collector changed canvas geometry; live resize is not supported, keeping the current framebuffer"
                    );
                }
                info!(collector_address, "Reconnected to collector");
                return (stream, config);
            }
            Err(error) => {
                warn!(collector_address, %error, "Failed to reconnect to collector, retrying");
                tokio::time::sleep(RECONNECT_BACKOFF).await;
            }
        }
    }
}

async fn write_handshake<W: AsyncWrite + Unpin>(writer: &mut W) -> io::Result<()> {
    writer.write_u32_le(MAGIC).await?;
    writer.flush().await
}

async fn read_handshake<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<()> {
    let magic = reader.read_u32_le().await?;
    if magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected protocol magic {magic:#010x}, expected {MAGIC:#010x}"),
        ));
    }

    Ok(())
}

/// Writes a length-delimited, postcard-encoded control message.
async fn write_message<W: AsyncWrite + Unpin, M: Serialize>(
    writer: &mut W,
    message: &M,
) -> io::Result<()> {
    let bytes = postcard::to_allocvec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control message too large"))?;

    writer.write_u32_le(len).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Reads a length-delimited, postcard-encoded control message.
async fn read_message<R: AsyncRead + Unpin, M: DeserializeOwned>(reader: &mut R) -> io::Result<M> {
    let len = reader.read_u32_le().await? as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control message of {len} bytes exceeds the {MAX_MESSAGE_SIZE} byte cap"),
        ));
    }

    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).await?;
    postcard::from_bytes(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
