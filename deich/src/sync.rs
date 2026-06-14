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

use std::{io, time::Duration};

use breakwater_parser::{
    FrameBuffer, TimeTrackingFrameBuffer, TimeTrackingPixel, get_current_ns_since_unix_epoch,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use uuid::Uuid;

/// Identifies the deich sync protocol on the wire ("deic" in ASCII). The collector and workers are
/// always deployed together, so a single magic to reject obviously-wrong connections is enough; we
/// don't bother with version negotiation.
const MAGIC: u32 = 0x6465_6963;

/// Upper bound on a (length-delimited) control message, to reject garbage before allocating.
/// Control messages are tiny; the big framebuffer blob is sent raw, outside this path.
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

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

    /// A framebuffer frame. On the wire this is immediately followed by
    /// [`WorkerConfig::frame_size_bytes`] raw framebuffer bytes (not part of this serialized
    /// message). `frame_number` is the scheduled slot this frame is for (see [`FrameSchedule`]);
    /// every node derives it from the same wall-clock schedule, so the collector knows which slot a
    /// frame belongs to. The per-pixel write timestamps live in the blob itself (absolute, set by
    /// the framebuffer), so no base needs to travel with the frame.
    Frame { frame_number: u64 },
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

    /// Wall-clock duration of `frames` frame slots.
    pub fn duration_of_frames(&self, frames: u64) -> Duration {
        Duration::from_nanos(frames.saturating_mul(self.frame_period_ns))
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

    /// The collector's startup time (ns since the UNIX epoch). Workers use it as the zero point for
    /// their per-pixel timestamps, so every worker's timestamps are mutually comparable at the
    /// collector. A 40-bit µs offset from here lasts ~12.7 days of collector uptime.
    pub epoch_ns_since_unix_epoch: u64,
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

/// Collector side: read a frame's `frame_number`. The caller must then read exactly
/// [`WorkerConfig::frame_size_bytes`] bytes with [`receive_frame_body`] before the next message, or
/// discard them — splitting the two lets the caller decide whether to keep the blob.
pub async fn receive_frame_number<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<u64> {
    match read_message(reader).await? {
        WorkerMessage::Frame { frame_number } => Ok(frame_number),
        WorkerMessage::Hello { worker_id } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a frame but got another hello from {worker_id}"),
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
/// Streams frames aligned to the shared [`FrameSchedule`] until the connection fails, returning the
/// error. The worker treats that as a signal to tear down and start a fresh session (reconnect, get
/// new config, rebuild the framebuffer), so there is no reconnection logic here.
///
/// Each iteration sleeps until the current slot ends and sends the framebuffer, tagged with the
/// just-completed slot number. Per-pixel timestamps are absolute (set by the framebuffer at write
/// time), so there's nothing to re-base — the worker only has to pick the right slot number, which
/// it recomputes from the clock so a worker that falls behind simply skips missed slots.
pub async fn sync_framebuffer(
    fb: &TimeTrackingFrameBuffer,
    stream: &mut TcpStream,
    config: WorkerConfig,
) -> io::Result<()> {
    let schedule = FrameSchedule::new(config.sync_fps);
    let mut frame_number = schedule.frame_number_at(get_current_ns_since_unix_epoch());

    loop {
        // Sleep until the current slot ends.
        let slot_end = schedule.frame_start_ns(frame_number + 1);
        let now = get_current_ns_since_unix_epoch();
        tokio::time::sleep(Duration::from_nanos(slot_end.saturating_sub(now))).await;

        write_message(stream, &WorkerMessage::Frame { frame_number }).await?;
        stream.write_all(fb.as_bytes()).await?;
        stream.flush().await?;

        // Advance to the current slot, skipping any we fell behind on, always moving forward.
        frame_number = schedule
            .frame_number_at(get_current_ns_since_unix_epoch())
            .max(frame_number + 1);
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
