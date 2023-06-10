use crate::{
    framebuffer::FrameBuffer,
    parser::{parse_pixelflut_commands, ParserState, PARSER_LOOKAHEAD},
    statistics::StatisticsEvent,
};
use log::{debug, info};
use std::{
    cmp::min,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc::Sender,
    time::Instant,
};

const NETWORK_BUFFER_SIZE: usize = 256_000;
// Every client connection spawns a new thread, so we need to limit the number of stat events we send
const STATISTICS_REPORT_INTERVAL: Duration = Duration::from_millis(250);

pub struct Network {
    listen_address: String,
    fb: Arc<FrameBuffer>,
    statistics_tx: Sender<StatisticsEvent>,
}

impl Network {
    pub fn new(
        listen_address: &str,
        fb: Arc<FrameBuffer>,
        statistics_tx: Sender<StatisticsEvent>,
    ) -> Self {
        Network {
            listen_address: listen_address.to_string(),
            fb,
            statistics_tx,
        }
    }

    pub async fn listen(&self) -> tokio::io::Result<()> {
        let listener = TcpListener::bind(&self.listen_address).await?;
        info!("Started Pixelflut server on {}", self.listen_address);

        loop {
            let (socket, socket_addr) = listener.accept().await?;
            // If you connect via IPv4 you often show up as embedded inside an IPv6 address
            // Extracting the embedded information here, so we get the real (TM) address
            let ip = ip_to_canonical(socket_addr.ip());

            let fb_for_thread = Arc::clone(&self.fb);
            let statistics_tx_for_thread = self.statistics_tx.clone();
            tokio::spawn(async move {
                handle_connection(socket, ip, fb_for_thread, statistics_tx_for_thread).await;
            });
        }
    }
}

pub async fn handle_connection(
    mut stream: impl AsyncReadExt + AsyncWriteExt + Unpin,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_tx: Sender<StatisticsEvent>,
) {
    debug!("Handling connection from {ip}");

    statistics_tx
        .send(StatisticsEvent::ConnectionCreated { ip })
        .await
        .expect("Statistics channel disconnected");

    // TODO: Try performance of Vec<> on heap instead of stack. Also bigger buffer
    let mut buffer = [0u8; NETWORK_BUFFER_SIZE];
    // Number bytes left over **on the first bytes of the buffer** from the previous loop iteration
    let mut leftover_bytes_in_buffer = 0;

    // We have to keep the some things - such as connection offset - for the whole connection lifetime, so let's define them here
    let mut parser_state = ParserState::default();

    // If we send e.g. an StatisticsEvent::BytesRead for every time we read something from the socket the statistics thread would go crazy.
    // Instead we bulk the statistics and send them pre-aggregated.
    let mut last_statistics = Instant::now();
    let mut statistics_bytes_read: u64 = 0;

    loop {
        // Fill the buffer up with new data from the socket
        // If there are any bytes left over from the previous loop iteration leave them as is and but the new data behind
        let bytes_read = match stream
            .read(&mut buffer[leftover_bytes_in_buffer..NETWORK_BUFFER_SIZE - PARSER_LOOKAHEAD])
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
                .expect("Statistics channel disconnected");
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
            for i in &mut buffer[data_end..data_end + PARSER_LOOKAHEAD] {
                *i = 0;
            }

            parser_state = parse_pixelflut_commands(
                &buffer[..data_end + PARSER_LOOKAHEAD],
                &fb,
                &mut stream,
                parser_state,
            )
            .await;

            // IMPORTANT: We have to subtract 1 here, as e.g. we have "PX 0 0\n" data_end is 7 and parser_state.last_byte_parsed is 6.
            // This happens, because last_byte_parsed is an index starting at 0, so index 6 is from an array of length 7
            leftover_bytes_in_buffer = data_end - parser_state.last_byte_parsed() - 1;

            // There is no need to leave anything longer than a command can take
            // This prevents malicious clients from sending gibberish and the buffer not getting drained
            leftover_bytes_in_buffer = min(leftover_bytes_in_buffer, PARSER_LOOKAHEAD);
        }

        if leftover_bytes_in_buffer > 0 {
            // We need to move the leftover bytes to the beginning of the buffer so that the next loop iteration con work on them
            buffer.copy_within(
                parser_state.last_byte_parsed() + 1
                    ..parser_state.last_byte_parsed() + 1 + leftover_bytes_in_buffer,
                0,
            );
        }
    }

    statistics_tx
        .send(StatisticsEvent::ConnectionClosed { ip })
        .await
        .expect("Statistics channel disconnected");
}

/// TODO: Switch to official ip.to_canonical() method when it is stable. **If** it gets stable sometime ;)
/// See <https://doc.rust-lang.org/std/net/enum.IpAddr.html#method.to_canonical>
fn ip_to_canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.octets() {
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => {
                IpAddr::V4(Ipv4Addr::new(a, b, c, d))
            }
            _ => ip,
        },
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::helpers::MockTcpStream;
    use rstest::{fixture, rstest};
    use std::time::Duration;
    use tokio::sync::mpsc::{self, Receiver};

    #[fixture]
    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    }

    #[fixture]
    fn fb() -> Arc<FrameBuffer> {
        Arc::new(FrameBuffer::new(1920, 1080))
    }

    #[fixture]
    fn statistics_channel() -> (Sender<StatisticsEvent>, Receiver<StatisticsEvent>) {
        mpsc::channel(10000)
    }

    #[rstest]
    #[timeout(Duration::from_secs(1))]
    #[case("", "")]
    #[case("\n", "")]
    #[case("not a pixelflut command", "")]
    #[case("not a pixelflut command with newline\n", "")]
    #[case("SIZE", "SIZE 1920 1080\n")]
    #[case("SIZE\n", "SIZE 1920 1080\n")]
    #[case("SIZE\nSIZE\n", "SIZE 1920 1080\nSIZE 1920 1080\n")]
    #[case("SIZE", "SIZE 1920 1080\n")]
    #[case("HELP", std::str::from_utf8(crate::parser::HELP_TEXT).unwrap())]
    #[case("HELP\n", std::str::from_utf8(crate::parser::HELP_TEXT).unwrap())]
    #[case("bla bla bla\nSIZE\nblub\nbla", "SIZE 1920 1080\n")]
    #[tokio::test]
    async fn test_correct_responses_to_general_commands(
        #[case] input: &str,
        #[case] expected: &str,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics_channel: (Sender<StatisticsEvent>, Receiver<StatisticsEvent>),
    ) {
        let mut stream = MockTcpStream::from_input(input);
        handle_connection(&mut stream, ip, fb, statistics_channel.0).await;

        assert_eq!(expected, stream.get_output());
    }

    #[rstest]
    // Without alpha
    #[case("PX 0 0 ffffff\nPX 0 0\n", "PX 0 0 ffffff\n")]
    #[case("PX 0 0 abcdef\nPX 0 0\n", "PX 0 0 abcdef\n")]
    #[case("PX 0 42 abcdef\nPX 0 42\n", "PX 0 42 abcdef\n")]
    #[case("PX 42 0 abcdef\nPX 42 0\n", "PX 42 0 abcdef\n")]
    // With alpha
    // TODO: At the moment alpha channel is not supported and silently ignored (pixels are painted with 0% transparency)
    #[case("PX 0 0 ffffffaa\nPX 0 0\n", "PX 0 0 ffffff\n")]
    #[case("PX 0 0 abcdefaa\nPX 0 0\n", "PX 0 0 abcdef\n")]
    #[case("PX 0 1 abcdefaa\nPX 0 1\n", "PX 0 1 abcdef\n")]
    #[case("PX 1 0 abcdefaa\nPX 1 0\n", "PX 1 0 abcdef\n")]
    // Tests invalid bounds
    #[case("PX 9999 0 abcdef\nPX 9999 0\n", "")] // Parsable but outside screen size
    #[case("PX 0 9999 abcdef\nPX 9999 0\n", "")]
    #[case("PX 9999 9999 abcdef\nPX 9999 9999\n", "")]
    #[case("PX 99999 0 abcdef\nPX 0 99999\n", "")] // Not even parsable because to many digits
    #[case("PX 0 99999 abcdef\nPX 0 99999\n", "")]
    #[case("PX 99999 99999 abcdef\nPX 99999 99999\n", "")]
    // Test invalid inputs
    #[case("PX 0 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("PX 0 1 2 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("PX -1 0 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("bla bla bla\nPX 0 0\n", "PX 0 0 000000\n")]
    // Test offset
    #[case(
        "OFFSET 10 10\nPX 0 0 ffffff\nPX 0 0\nPX 42 42\n",
        "PX 0 0 ffffff\nPX 42 42 000000\n"
    )] // The get pixel result is also offseted
    #[case("OFFSET 0 0\nPX 0 42 abcdef\nPX 0 42\n", "PX 0 42 abcdef\n")]
    #[tokio::test]
    async fn test_setting_pixel(
        #[case] input: &str,
        #[case] expected: &str,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics_channel: (Sender<StatisticsEvent>, Receiver<StatisticsEvent>),
    ) {
        let mut stream = MockTcpStream::from_input(input);
        handle_connection(&mut stream, ip, fb, statistics_channel.0).await;

        assert_eq!(expected, stream.get_output());
    }

    #[rstest]
    #[case(5, 5, 0, 0)]
    #[case(6, 6, 0, 0)]
    #[case(7, 7, 0, 0)]
    #[case(8, 8, 0, 0)]
    #[case(9, 9, 0, 0)]
    #[case(10, 10, 0, 0)]
    #[case(10, 10, 100, 200)]
    #[case(10, 10, 510, 520)]
    #[case(100, 100, 0, 0)]
    #[case(100, 100, 300, 400)]
    #[case(479, 361, 721, 391)]
    #[case(500, 500, 0, 0)]
    #[case(500, 500, 300, 400)]
    #[case(fb().get_width(), fb().get_height(), 0, 0)]
    #[case(fb().get_width() - 1, fb().get_height() - 1, 1, 1)]
    #[tokio::test]
    async fn test_drawing_rect(
        #[case] width: usize,
        #[case] height: usize,
        #[case] offset_x: usize,
        #[case] offset_y: usize,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics_channel: (Sender<StatisticsEvent>, Receiver<StatisticsEvent>),
    ) {
        let mut color: u32 = 0;
        let mut fill_commands = String::new();
        let mut read_commands = String::new();
        let mut combined_commands = String::new();
        let mut combined_commands_expected = String::new();
        let mut read_other_pixels_commands = String::new();
        let mut read_other_pixels_commands_expected = String::new();

        for x in 0..fb.get_width() {
            for y in 0..height {
                // Inside the rect
                if x >= offset_x && x <= offset_x + width && y >= offset_y && y <= offset_y + height
                {
                    fill_commands += &format!("PX {x} {y} {color:06x}\n");
                    read_commands += &format!("PX {x} {y}\n");

                    color += 1; // Use another color for the next test case
                    combined_commands += &format!("PX {x} {y} {color:06x}\nPX {x} {y}\n");
                    combined_commands_expected += &format!("PX {x} {y} {color:06x}\n");

                    color += 1;
                } else {
                    // Non touched pixels must remain black
                    read_other_pixels_commands += &format!("PX {x} {y}\n");
                    read_other_pixels_commands_expected += &format!("PX {x} {y} 000000\n");
                }
            }
        }

        // Color the pixels
        let mut stream = MockTcpStream::from_input(&fill_commands);
        handle_connection(
            &mut stream,
            ip,
            Arc::clone(&fb),
            statistics_channel.0.clone(),
        )
        .await;
        assert_eq!("", stream.get_output());

        // Read the pixels again
        let mut stream = MockTcpStream::from_input(&read_commands);
        handle_connection(
            &mut stream,
            ip,
            Arc::clone(&fb),
            statistics_channel.0.clone(),
        )
        .await;
        assert_eq!(fill_commands, stream.get_output());

        // We can also do coloring and reading in a single connection
        let mut stream = MockTcpStream::from_input(&combined_commands);
        handle_connection(
            &mut stream,
            ip,
            Arc::clone(&fb),
            statistics_channel.0.clone(),
        )
        .await;
        assert_eq!(combined_commands_expected, stream.get_output());

        // Check that nothing else was colored
        let mut stream = MockTcpStream::from_input(&read_other_pixels_commands);
        handle_connection(
            &mut stream,
            ip,
            Arc::clone(&fb),
            statistics_channel.0.clone(),
        )
        .await;
        assert_eq!(read_other_pixels_commands_expected, stream.get_output());
    }
}
