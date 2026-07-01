//! Worker ↔ collector sync protocol.
//!
//! A worker connects to the collector and announces itself with a handshake (magic) plus a
//! [`WorkerMessage::Hello`] carrying its persistent UUID. The collector replies with a
//! [`CollectorMessage::Config`] — it owns canvas geometry, frame rate and the timestamp epoch — and
//! the worker then streams full framebuffers (and periodic statistics) until the connection drops.
//!
//! Control messages are serialized with [`postcard`] and length-delimited (a little-endian `u32`
//! length prefix), so the message set can grow into richer enums over time. A framebuffer blob
//! (~12 MB) is deliberately *not* serialized: a [`WorkerMessage::Framebuffer`] marker is immediately
//! followed on the wire by [`WorkerConfig::frame_size_bytes`] raw bytes. [`Connection`] owns that
//! "marker then raw blob" contract, so no caller has to remember it.

use std::{io, net::SocketAddr, time::Duration};

use breakwater::statistics::StatisticsInformationEvent;
use breakwater_parser::{TimeTrackingPixel, pixels_as_bytes_mut};
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

    /// Marker that a full framebuffer follows on the wire as [`WorkerConfig::frame_size_bytes`] raw
    /// bytes (not part of this serialized message). The per-pixel write timestamps live in the blob
    /// itself (absolute, relative to the shared epoch), so nothing else needs to travel with it.
    Framebuffer,

    /// A periodic statistics snapshot, already aggregated per IP by the worker (~once per second)
    /// and cumulative within this connection's session. Self-contained — no raw bytes follow.
    Statistics(StatisticsInformationEvent),
}

/// Configuration the collector hands to each worker. The collector is the single source of truth for
/// canvas geometry, frame rate and the timestamp epoch; workers configure themselves from it. Extend
/// this when workers need more, e.g. their offset within a larger canvas.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,

    /// The collector's startup time (ns since the UNIX epoch). Workers use it as the zero point for
    /// their per-pixel timestamps, so every worker's timestamps are mutually comparable at the
    /// collector. A 40-bit µs offset from here lasts ~12.7 days of collector uptime.
    pub epoch_ns_since_unix_epoch: u64,
}

impl WorkerConfig {
    /// Number of pixels in one framebuffer.
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Number of bytes in one framebuffer blob.
    pub fn frame_size_bytes(&self) -> usize {
        self.pixel_count() * size_of::<TimeTrackingPixel>()
    }
}

/// The interval between framebuffer pushes (worker side) and renders (collector side) for a given
/// frame rate. Each node just runs a local timer at this cadence — there is no shared wall-clock
/// schedule and no cross-node clock coupling; ordering is resolved entirely by the per-pixel
/// timestamps in the framebuffer.
pub fn frame_period(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1)))
}

/// What a worker sent us: either a full framebuffer (already read off the wire into an owned buffer)
/// or a statistics snapshot. The one-shot [`WorkerMessage::Hello`] is consumed at [`accept`] time.
pub enum WorkerData {
    Framebuffer(Vec<TimeTrackingPixel>),
    Statistics(StatisticsInformationEvent),
}

/// A length-delimited, postcard-framed connection over some byte stream. It owns the deich wire
/// format — the magic handshake, control-message framing, and the raw framebuffer-blob contract — so
/// the worker and collector only ever deal in typed messages.
pub struct Connection<S> {
    stream: S,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }

    /// Collector side: send the worker its config in reply to the hello.
    pub async fn send_config(&mut self, config: WorkerConfig) -> io::Result<()> {
        self.send_message(&CollectorMessage::Config(config)).await
    }

    /// Worker side: push a full framebuffer — the marker immediately followed by its raw bytes,
    /// flushed together so a statistics message can never slip between the two.
    pub async fn send_framebuffer(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write_message(&WorkerMessage::Framebuffer).await?;
        self.stream.write_all(bytes).await?;
        self.stream.flush().await
    }

    /// Worker side: send a statistics snapshot.
    pub async fn send_statistics(&mut self, event: StatisticsInformationEvent) -> io::Result<()> {
        self.send_message(&WorkerMessage::Statistics(event)).await
    }

    /// Collector side: read the next framebuffer or statistics message. For a framebuffer the raw
    /// blob is read here into a fresh `pixel_count`-long buffer (allocated only when a framebuffer
    /// actually arrives), so callers never touch the "raw bytes follow the marker" contract. A
    /// second [`WorkerMessage::Hello`] mid-stream is a protocol error.
    pub async fn recv_worker_data(&mut self, pixel_count: usize) -> io::Result<WorkerData> {
        match self.recv_message::<WorkerMessage>().await? {
            WorkerMessage::Framebuffer => {
                let mut frame = vec![TimeTrackingPixel::default(); pixel_count];
                self.stream
                    .read_exact(pixels_as_bytes_mut(&mut frame))
                    .await?;
                Ok(WorkerData::Framebuffer(frame))
            }
            WorkerMessage::Statistics(event) => Ok(WorkerData::Statistics(event)),
            WorkerMessage::Hello { worker_id } => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected second hello (worker id {worker_id}) mid-stream"),
            )),
        }
    }

    /// Writes a length-delimited, postcard-encoded control message and flushes it.
    async fn send_message<M: Serialize>(&mut self, message: &M) -> io::Result<()> {
        self.write_message(message).await?;
        self.stream.flush().await
    }

    /// Writes a length-delimited, postcard-encoded control message *without* flushing (so a
    /// framebuffer blob can be appended and the pair flushed as one).
    async fn write_message<M: Serialize>(&mut self, message: &M) -> io::Result<()> {
        let bytes = postcard::to_allocvec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let len = u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control message too large"))?;

        self.stream.write_u32_le(len).await?;
        self.stream.write_all(&bytes).await
    }

    /// Reads a length-delimited, postcard-encoded control message.
    async fn recv_message<M: DeserializeOwned>(&mut self) -> io::Result<M> {
        let len = self.stream.read_u32_le().await? as usize;
        if len > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control message of {len} bytes exceeds the {MAX_MESSAGE_SIZE} byte cap"),
            ));
        }

        let mut bytes = vec![0; len];
        self.stream.read_exact(&mut bytes).await?;
        postcard::from_bytes(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    async fn send_magic(&mut self) -> io::Result<()> {
        self.stream.write_u32_le(MAGIC).await?;
        self.stream.flush().await
    }

    async fn expect_magic(&mut self) -> io::Result<()> {
        let magic = self.stream.read_u32_le().await?;
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected protocol magic {magic:#010x}, expected {MAGIC:#010x}"),
            ));
        }

        Ok(())
    }
}

/// Worker side: connect to the collector, announce ourselves with `worker_id`, and read the config
/// the collector replies with.
pub async fn connect(
    collector_address: SocketAddr,
    worker_id: Uuid,
) -> io::Result<(Connection<TcpStream>, WorkerConfig)> {
    let mut connection = Connection::new(TcpStream::connect(collector_address).await?);
    connection.send_magic().await?;
    connection
        .send_message(&WorkerMessage::Hello { worker_id })
        .await?;

    let CollectorMessage::Config(config) = connection.recv_message().await?;
    Ok((connection, config))
}

/// Collector side: validate the handshake and read the worker's [`WorkerMessage::Hello`], returning
/// the connection and the worker's UUID so it can be attributed.
pub async fn accept(stream: TcpStream) -> io::Result<(Connection<TcpStream>, Uuid)> {
    let mut connection = Connection::new(stream);
    connection.expect_magic().await?;

    let worker_id = match connection.recv_message::<WorkerMessage>().await? {
        WorkerMessage::Hello { worker_id } => worker_id,
        WorkerMessage::Framebuffer => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a hello message but got a framebuffer",
            ));
        }
        WorkerMessage::Statistics(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a hello message but got statistics",
            ));
        }
    };

    Ok((connection, worker_id))
}

#[cfg(test)]
mod tests {
    use breakwater_parser::{FrameBuffer, TimeTrackingFrameBuffer};

    use super::*;

    /// A framebuffer push and a statistics snapshot survive the wire round-trip, and the collector
    /// reads each back as the right [`WorkerData`] variant (with the blob contract handled for it).
    #[tokio::test]
    async fn framebuffer_and_statistics_round_trip() {
        let (client, server) = tokio::io::duplex(1 << 16);
        let mut worker = Connection::new(client);
        let mut collector = Connection::new(server);

        // A 2x1 framebuffer with one pixel set, so we have a real blob to compare.
        let fb = TimeTrackingFrameBuffer::new(2, 1, 0);
        fb.set(0, 0, 0x00_00ff, 42);

        worker.send_framebuffer(fb.as_raw_bytes()).await.unwrap();
        worker
            .send_statistics(StatisticsInformationEvent {
                bytes: 1234,
                ..Default::default()
            })
            .await
            .unwrap();

        match collector.recv_worker_data(2).await.unwrap() {
            WorkerData::Framebuffer(frame) => {
                assert_eq!(frame.len(), 2);
                assert_eq!(frame[0].rgb(), 0x00_00ff);
                assert_eq!(frame[0].timestamp(), 42);
            }
            WorkerData::Statistics(_) => panic!("expected a framebuffer first"),
        }

        match collector.recv_worker_data(2).await.unwrap() {
            WorkerData::Statistics(event) => assert_eq!(event.bytes, 1234),
            WorkerData::Framebuffer(_) => panic!("expected statistics second"),
        }
    }
}
