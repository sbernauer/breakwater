use std::{cmp::min, net::IpAddr, sync::Arc, time::Duration};

use breakwater_core::framebuffer::FrameBuffer;
use breakwater_parser::{original::OriginalParser, Parser, ParserError};
use log::{debug, info};
use snafu::{ResultExt, Snafu};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    time::Instant,
};

use crate::statistics::StatisticsEvent;

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

    #[snafu(display("Failed to write to statistics channel"))]
    WriteToStatisticsChannel {
        source: mpsc::error::SendError<StatisticsEvent>,
    },

    #[snafu(display("Failed to parse Pixelflut commands"))]
    ParsePixelflutCommands { source: ParserError },
}

pub struct Server {
    // listen_address: String,
    listener: TcpListener,
    fb: Arc<FrameBuffer>,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    network_buffer_size: usize,
}

impl Server {
    pub async fn new(
        listen_address: &str,
        fb: Arc<FrameBuffer>,
        statistics_tx: mpsc::Sender<StatisticsEvent>,
        network_buffer_size: usize,
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
        })
    }

    pub async fn start(&self) -> Result<(), Error> {
        loop {
            let (socket, socket_addr) = self
                .listener
                .accept()
                .await
                .context(AcceptNewClientConnectionSnafu)?;
            // If you connect via IPv4 you often show up as embedded inside an IPv6 address
            // Extracting the embedded information here, so we get the real (TM) address
            let ip = socket_addr.ip().to_canonical();

            let fb_for_thread = Arc::clone(&self.fb);
            let statistics_tx_for_thread = self.statistics_tx.clone();
            let network_buffer_size = self.network_buffer_size;
            tokio::spawn(async move {
                handle_connection(
                    socket,
                    ip,
                    fb_for_thread,
                    statistics_tx_for_thread,
                    network_buffer_size,
                )
                .await
            });
        }
    }
}

pub async fn handle_connection(
    mut stream: impl AsyncReadExt + AsyncWriteExt + Send + Unpin,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_tx: mpsc::Sender<StatisticsEvent>,
    network_buffer_size: usize,
) -> Result<(), Error> {
    debug!("Handling connection from {ip}");

    statistics_tx
        .send(StatisticsEvent::ConnectionCreated { ip })
        .await
        .context(WriteToStatisticsChannelSnafu)?;

    let mut buffer = vec![0u8; network_buffer_size];
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
        // If there are any bytes left over from the previous loop iteration leave them as is and but the new data behind
        let bytes_read = match stream
            .read(&mut buffer[leftover_bytes_in_buffer..network_buffer_size - parser_lookahead])
            .await
        {
            Ok(bytes_read) => bytes_read,
            Err(_) => {
                break;
            }
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

            let last_byte_parsed = parser
                .parse(&buffer[..data_end + parser_lookahead], &mut stream)
                .await
                .context(ParsePixelflutCommandsSnafu)?;

            // IMPORTANT: We have to subtract 1 here, as e.g. we have "PX 0 0\n" data_end is 7 and parser_state.last_byte_parsed is 6.
            // This happens, because last_byte_parsed is an index starting at 0, so index 6 is from an array of length 7
            leftover_bytes_in_buffer = data_end.saturating_sub(last_byte_parsed).saturating_sub(1);

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

    Ok(())
}
