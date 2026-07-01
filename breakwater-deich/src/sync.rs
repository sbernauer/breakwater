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

use std::{io, net::SocketAddr, time::Duration};

use breakwater::statistics::StatisticsInformationEvent;
use breakwater_parser::{
    TimeTrackingFrameBuffer, TimeTrackingPixel, get_current_ns_since_unix_epoch,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::broadcast,
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

    /// A periodic statistics snapshot, already aggregated per IP by the worker's statistics
    /// aggregator (roughly once per second). The collector merges these across all connected
    /// workers for display. Unlike [`WorkerMessage::Frame`] this is self-contained — no raw bytes
    /// follow it on the wire.
    Statistics(StatisticsInformationEvent),
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
    pub fn frame_number_at(self, ns_since_unix_epoch: u64) -> u64 {
        ns_since_unix_epoch / self.frame_period_ns
    }

    /// The UNIX-epoch nanosecond timestamp at which `frame_number` begins.
    pub fn frame_start_ns(self, frame_number: u64) -> u64 {
        frame_number * self.frame_period_ns
    }

    /// How many whole frames `duration` spans, rounded up. Used to size delay margins.
    pub fn frames_spanning(self, duration: Duration) -> u64 {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        ns.div_ceil(self.frame_period_ns)
    }

    /// Wall-clock duration of `frames` frame slots.
    pub fn duration_of_frames(self, frames: u64) -> Duration {
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
    collector_address: SocketAddr,
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
        WorkerMessage::Statistics(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a hello message but got statistics",
        )),
    }
}

/// Collector side: read the next message from a worker. For a [`WorkerMessage::Frame`] the caller
/// must then read exactly [`WorkerConfig::frame_size_bytes`] bytes with [`receive_frame_body`]
/// before the next message (or discard them) — splitting the two lets the caller decide whether to
/// keep the blob. A [`WorkerMessage::Statistics`] is self-contained, and a second
/// [`WorkerMessage::Hello`] mid-stream is a protocol error for the caller to reject.
pub async fn receive_worker_message<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> io::Result<WorkerMessage> {
    read_message(reader).await
}

/// Collector side: send the worker its config in reply to the hello.
pub async fn send_config<W: AsyncWrite + Unpin>(
    writer: &mut W,
    config: WorkerConfig,
) -> io::Result<()> {
    write_message(writer, &CollectorMessage::Config(config)).await
}

/// Collector side: read a frame's raw blob into `pixels` (which must be
/// [`WorkerConfig::frame_size_bytes`] long), following a [`WorkerMessage::Frame`] from
/// [`receive_worker_message`].
pub async fn receive_frame_body<R: AsyncRead + Unpin>(
    reader: &mut R,
    pixels: &mut [u8],
) -> io::Result<()> {
    reader.read_exact(pixels).await?;
    Ok(())
}

/// Worker side: stream frames and periodic statistics to the collector over the one connection,
/// until it fails (then the error is returned).
///
/// Both kinds of message share this connection *and this task*, so their writes can never
/// interleave — a statistics message can't slip between a [`WorkerMessage::Frame`] header and its
/// raw blob. The worker treats a returned error as a signal to tear down and start a fresh session
/// (reconnect, get new config, rebuild the framebuffer), so there is no reconnection logic here.
///
/// `stream`/`config` are the live connection and config from the initial [`connect`]. Frames are
/// aligned to the shared [`FrameSchedule`]: each tick sleeps until the current slot ends and sends
/// the framebuffer, tagged with the just-completed slot number. Per-pixel timestamps are absolute
/// (set by the framebuffer at write time), so there's nothing to re-base — the worker only picks
/// the slot number, recomputed from the clock so a worker that falls behind simply skips missed
/// slots. Statistics snapshots arrive on `statistics_rx` (~once per second) and are forwarded as
/// they come.
pub async fn sync(
    fb: &TimeTrackingFrameBuffer,
    stream: &mut TcpStream,
    config: WorkerConfig,
    mut statistics_rx: broadcast::Receiver<StatisticsInformationEvent>,
) -> io::Result<()> {
    let schedule = FrameSchedule::new(config.sync_fps);
    let mut frame_number = schedule.frame_number_at(get_current_ns_since_unix_epoch());

    loop {
        // Sleep until the current slot ends, but wake early to forward a statistics snapshot.
        let slot_end = schedule.frame_start_ns(frame_number + 1);
        let now = get_current_ns_since_unix_epoch();
        let slot_sleep = tokio::time::sleep(Duration::from_nanos(slot_end.saturating_sub(now)));
        tokio::pin!(slot_sleep);

        tokio::select! {
            () = &mut slot_sleep => {
                write_message(stream, &WorkerMessage::Frame { frame_number }).await?;
                stream.write_all(fb.as_raw_bytes()).await?;
                stream.flush().await?;

                // Advance to the current slot, skipping any we fell behind on, always moving forward.
                frame_number = schedule
                    .frame_number_at(get_current_ns_since_unix_epoch())
                    .max(frame_number + 1);
            }
            event = statistics_rx.recv() => match event {
                Ok(event) => write_message(stream, &WorkerMessage::Statistics(event)).await?,
                // Lagged: snapshots are emitted ~once per second into a small buffer with a single
                // consumer, so lagging shouldn't happen; if it somehow does, skip the dropped ones.
                // Closed: the aggregator is gone, but the whole session is torn down at that point
                // (it runs as a sibling `select!` arm in the worker), so this is effectively
                // unreachable. Either way: keep streaming frames rather than failing the sync.
                Err(broadcast::error::RecvError::Lagged(_) | broadcast::error::RecvError::Closed) => {}
            },
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
