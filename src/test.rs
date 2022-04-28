#[cfg(test)]
mod test {
    use crate::network::{self, HELP_TEXT};
    use crate::{FrameBuffer, Statistics};
    use clap::lazy_static::lazy_static;
    use rstest::{fixture, rstest};
    use std::cmp::min;
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr};
    use std::str;
    use std::string::String;
    use std::sync::Arc;

    lazy_static! {
        pub static ref STATISTICS: Arc<Statistics> = Arc::new(Statistics::new());
    }

    #[fixture]
    fn fb() -> Arc<FrameBuffer> {
        Arc::new(FrameBuffer::new(1920, 1080))
    }

    #[fixture]
    fn statistics() -> Arc<Statistics> {
        // We need a single statistics object as otherwise it tries to register the same Prometheus metric multiple times
        Arc::clone(&STATISTICS)
    }

    #[fixture]
    fn ip(statistics: Arc<Statistics>) -> IpAddr {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        // We need to increase the connections for this IP, as this normally would be handled by code that is not part of this test
        // If we won't increase the connections, the HashMap will be missing the keys
        statistics.inc_connections(ip);
        ip
    }

    #[rstest]
    #[case("", "")]
    #[case("\n", "")]
    #[case("not a pixelflut command", "")]
    #[case("not a pixelflut command with newline\n", "")]
    #[case("SIZE", "SIZE 1920 1080\n")]
    #[case("SIZE\n", "SIZE 1920 1080\n")]
    #[case("SIZE\nSIZE\n", "SIZE 1920 1080\nSIZE 1920 1080\n")]
    #[case("HELP", str::from_utf8(HELP_TEXT).unwrap())]
    #[case("HELP\n", str::from_utf8(HELP_TEXT).unwrap())]
    fn test_correct_responses_to_general_commands(
        #[case] input: &str,
        #[case] expected: &str,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics: Arc<Statistics>,
    ) {
        let mut stream = MockTcpStream::from_input(input);
        network::handle_connection(&mut stream, ip, fb, statistics);

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
    #[case("PX 99999 0 abcdef\nPX 99999 0\n", "")] // Not even parsable because to many digits
    // Test invalid inputs
    #[case("PX 0 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("PX 0 1 2 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("PX -1 0 abcdef\nPX 0 0\n", "PX 0 0 000000\n")]
    #[case("bla bla bla\nPX 0 0\n", "PX 0 0 000000\n")]
    fn test_setting_pixel(
        #[case] input: &str,
        #[case] expected: &str,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics: Arc<Statistics>,
    ) {
        let mut stream = MockTcpStream::from_input(input);
        network::handle_connection(&mut stream, ip, fb, statistics);

        assert_eq!(expected, stream.get_output());
    }

    #[rstest]
    fn test_drawing_whole_screen(ip: IpAddr, fb: Arc<FrameBuffer>, statistics: Arc<Statistics>) {
        let mut color: u32 = 0;
        let mut input = String::new();
        let mut output = String::new();

        // TODO: Iterate over whole drawing surface when handle_connection implements a kind of ring buffer
        for x in 0..50 {
            for y in 0..50 {
                input += format!("PX {x} {y} {color:06x}\nPX {x} {y}\n").as_str();
                output += format!("PX {x} {y} {color:06x}\n").as_str();
                color += 1;
            }
        }

        let mut stream = MockTcpStream::from_input(&input);
        network::handle_connection(&mut stream, ip, fb, statistics);

        assert_eq!(output, stream.get_output());
    }

    #[derive(Debug)]
    struct MockTcpStream {
        read_data: Vec<u8>,
        write_data: Vec<u8>,
    }

    impl MockTcpStream {
        fn from_input(input: &str) -> Self {
            MockTcpStream {
                read_data: input.as_bytes().to_vec(),
                write_data: Vec::new(),
            }
        }

        fn get_output(self) -> String {
            String::from_utf8(self.write_data).unwrap()
        }
    }

    impl Read for MockTcpStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let size: usize = min(self.read_data.len(), buf.len());
            buf[..size].copy_from_slice(&self.read_data[..size]);

            self.read_data.drain(..size);
            Ok(size)
        }
    }

    impl Write for MockTcpStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.write_data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
