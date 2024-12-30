use std::alloc;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::{cmp::min, net::IpAddr, sync::Arc, time::Duration};

use breakwater_parser::{FrameBuffer, OriginalParser, Parser};
use log::{debug, info, warn};
use memadvise::{Advice, MemAdviseError};
use snafu::{ResultExt, Snafu};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    time::Instant,
};

use crate::statistics::StatisticsEvent;

const CONNECTION_DENIED_TEXT: &[u8] = b"Connection denied as connection limit is reached";

// Every client connection spawns a new thread, so we need to limit the number of stat events we send
const STATISTICS_REPORT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to bind to listen address {listen_address:?}"))]
    BindToListenAddress {
        source: std::io::Error,
        listen_address: String,
    },

    #[snafu(display("Failed to accept new client connection"))]
    AcceptNewClientConnection { source: std::io::Error },

    #[snafu(display("Failed to write to client connection"))]
    WriteToClientConnection { source: std::io::Error },

    #[snafu(display("Failed to write to statistics channel"))]
    WriteToStatisticsChannel {
        source: mpsc::error::SendError<StatisticsEvent>,
    },
}

pub struct Server<FB: FrameBuffer> {
    // listen_address: String,
    listener: TcpListener,
    fb: Arc<FB>,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    network_buffer_size: usize,
    connections_per_ip: HashMap<IpAddr, u64>,
    max_connections_per_ip: Option<u64>,
}

impl<FB: FrameBuffer + Send + Sync + 'static> Server<FB> {
    pub async fn new(
        listen_address: &str,
        fb: Arc<FB>,
        statistics_tx: mpsc::Sender<StatisticsEvent>,
        network_buffer_size: usize,
        max_connections_per_ip: Option<u64>,
    ) -> Result<Self, Error> {
        let listener = TcpListener::bind(listen_address)
            .await
            .context(BindToListenAddressSnafu { listen_address })?;
        info!("Started Pixelflut server on {listen_address}");

        Ok(Self {
            listener,
            fb,
            statistics_tx,
            network_buffer_size,
            connections_per_ip: HashMap::new(),
            max_connections_per_ip,
        })
    }

    pub async fn start(&mut self) -> Result<(), Error> {
        let (connection_dropped_tx, mut connection_dropped_rx) =
            mpsc::unbounded_channel::<IpAddr>();
        let connection_dropped_tx = self.max_connections_per_ip.map(|_| connection_dropped_tx);

        let page_size = page_size::get();
        debug!("System has a page size of {page_size} bytes");

        loop {
            let (mut socket, socket_addr) = self
                .listener
                .accept()
                .await
                .context(AcceptNewClientConnectionSnafu)?;

            // If connections are unlimited, will execute one try_recv per new connection
            while let Ok(ip) = connection_dropped_rx.try_recv() {
                if let Entry::Occupied(mut o) = self.connections_per_ip.entry(ip) {
                    let connections = o.get_mut();
                    *connections -= 1;
                    if *connections == 0 {
                        o.remove_entry();
                    }
                }
            }

            // If you connect via IPv4 you often show up as embedded inside an IPv6 address
            // Extracting the embedded information here, so we get the real (TM) address
            let ip = socket_addr.ip().to_canonical();

            if let Some(limit) = self.max_connections_per_ip {
                let current_connections = self.connections_per_ip.entry(ip).or_default();
                if *current_connections < limit {
                    *current_connections += 1;
                } else {
                    self.statistics_tx
                        .send(StatisticsEvent::ConnectionDenied { ip })
                        .await
                        .context(WriteToStatisticsChannelSnafu)?;

                    // Only best effort, it's ok if this message get's missed
                    let _ = socket.write_all(CONNECTION_DENIED_TEXT).await;
                    // This can error if a connection is dropped prematurely, which is totally fine
                    let _ = socket.shutdown().await;
                    continue;
                }
            };

            let fb_for_thread = Arc::clone(&self.fb);
            let statistics_tx_for_thread = self.statistics_tx.clone();
            let network_buffer_size = self.network_buffer_size;
            let connection_dropped_tx_clone = connection_dropped_tx.clone();
            tokio::spawn(async move {
                handle_connection(
                    socket,
                    ip,
                    fb_for_thread,
                    statistics_tx_for_thread,
                    page_size,
                    network_buffer_size,
                    connection_dropped_tx_clone,
                )
                .await
            });
        }
    }
}

pub async fn handle_connection<FB: FrameBuffer>(
    mut stream: impl AsyncReadExt + AsyncWriteExt + Send + Unpin,
    ip: IpAddr,
    fb: Arc<FB>,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    page_size: usize,
    network_buffer_size: usize,
    connection_dropped_tx: Option<mpsc::UnboundedSender<IpAddr>>,
) -> Result<(), Error> {
    debug!("Handling connection from {ip}");

    statistics_tx
        .send(StatisticsEvent::ConnectionCreated { ip })
        .await
        .context(WriteToStatisticsChannelSnafu)?;

    let layout = alloc::Layout::from_size_align(network_buffer_size, page_size).unwrap();
    let ptr = unsafe { alloc::alloc(layout) };
    let buffer = unsafe { std::slice::from_raw_parts_mut(ptr, network_buffer_size) };
    let mut response_buf = Vec::new();

    if let Err(err) = memadvise::advise(buffer.as_ptr() as _, buffer.len(), Advice::Sequential) {
        // [`MemAdviseError`] does not implement Debug...
        let err = match err {
            MemAdviseError::NullAddress => "NullAddress",
            MemAdviseError::InvalidLength => "InvalidLength",
            MemAdviseError::UnalignedAddress => "UnalignedAddress",
            MemAdviseError::InvalidRange => "InvalidRange",
        };
        warn!("Failed to memadvise sequential read access for buffer to kernel. This should not effect any client connections, but might having some minor performance degration: {err}");
    }

    // Number bytes left over **on the first bytes of the buffer** from the previous loop iteration
    let mut leftover_bytes_in_buffer = 0;

    // Not using `ParserImplementation` to avoid the dynamic dispatch.
    // let mut parser = ParserImplementation::Simple(SimpleParser::new(fb));
    let mut parser = OriginalParser::new(fb);
    let parser_lookahead = parser.parser_lookahead();

    // If we send e.g. an StatisticsEvent::BytesRead for every time we read something from the socket the statistics thread would go crazy.
    // Instead we bulk the statistics and send them pre-aggregated.
    let mut last_statistics = Instant::now();
    let mut statistics_bytes_read: u64 = 0;

    loop {
        // Fill the buffer up with new data from the socket
        // If there are any bytes left over from the previous loop iteration leave them as is and put the new data behind
        let Ok(bytes_read) = stream
            .read(&mut buffer[leftover_bytes_in_buffer..network_buffer_size - parser_lookahead])
            .await
        else {
            break;
        };

        statistics_bytes_read += bytes_read as u64;
        if last_statistics.elapsed() > STATISTICS_REPORT_INTERVAL {
            statistics_tx
                // We use a blocking call here as we want to process the stats.
                // Otherwise the stats will lag behind resulting in weird spikes in bytes/s statistics.
                // As the statistics calculation should be trivial let's wait for it
                .send(StatisticsEvent::BytesRead {
                    ip,
                    bytes: statistics_bytes_read,
                })
                .await
                .context(WriteToStatisticsChannelSnafu)?;
            last_statistics = Instant::now();
            statistics_bytes_read = 0;
        }

        let data_end = leftover_bytes_in_buffer + bytes_read;
        if bytes_read == 0 {
            if leftover_bytes_in_buffer == 0 {
                // We read no data and the previous loop did consume all data
                // Nothing to do here, closing connection
                break;
            }

            // No new data from socket, read to the end and everything should be fine
            leftover_bytes_in_buffer = 0;
        } else {
            // We have read some data, process it

            // We need to zero the PARSER_LOOKAHEAD bytes, so the parser does not detect any command left over from a previous loop iteration
            for i in &mut buffer[data_end..data_end + parser_lookahead] {
                *i = 0;
            }

            let last_byte_parsed =
                parser.parse(&buffer[..data_end + parser_lookahead], &mut response_buf);

            if !response_buf.is_empty() {
                stream
                    .write_all(&response_buf)
                    .await
                    .context(WriteToClientConnectionSnafu)?;
                response_buf.clear();
            }

            // IMPORTANT: We have to subtract 1 here, as e.g. we have "PX 0 0\n" data_end is 7 and parser_state.last_byte_parsed is 6.
            // This happens, because last_byte_parsed is an index starting at 0, so index 6 is from an array of length 7
            leftover_bytes_in_buffer = data_end.saturating_sub(last_byte_parsed).saturating_sub(1);

            // dbg!(
            //     buffer.len(),
            //     last_byte_parsed,
            //     leftover_bytes_in_buffer,
            //     &buffer[..25],
            //     &buffer[last_byte_parsed.saturating_sub(5)..last_byte_parsed],
            //     &buffer[buffer.len().saturating_sub(5)..]
            // );

            // There is no need to leave anything longer than a command can take
            // This prevents malicious clients from sending gibberish and the buffer not getting drained
            leftover_bytes_in_buffer = min(leftover_bytes_in_buffer, parser_lookahead);

            if leftover_bytes_in_buffer > 0 {
                // We need to move the leftover bytes to the beginning of the buffer so that the next loop iteration con work on them
                buffer.copy_within(
                    last_byte_parsed + 1..last_byte_parsed + 1 + leftover_bytes_in_buffer,
                    0,
                );
            }
        }
    }

    statistics_tx
        .send(StatisticsEvent::ConnectionClosed { ip })
        .await
        .context(WriteToStatisticsChannelSnafu)?;

    if let Some(tx) = connection_dropped_tx {
        // Will fail if the server thread ends before the client thread
        let _ = tx.send(ip);
    }

    let _ = memadvise::advise(buffer.as_ptr() as _, buffer.len(), Advice::DontNeed);

    Ok(())
}
