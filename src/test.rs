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
    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
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
    #[case(fb().width, fb().height, 0, 0)]
    #[case(fb().width - 1, fb().height - 1, 1, 1)]
    fn test_drawing_rect(
        #[case] width: usize,
        #[case] height: usize,
        #[case] offset_x: usize,
        #[case] offset_y: usize,
        ip: IpAddr,
        fb: Arc<FrameBuffer>,
        statistics: Arc<Statistics>,
    ) {
        let mut color: u32 = 0;
        let mut fill_commands = String::new();
        let mut read_commands = String::new();
        let mut combined_commands = String::new();
        let mut combined_commands_expected = String::new();

        for x in offset_x..(offset_x + width) {
            for y in offset_y..(offset_y + height) {
                fill_commands += &format!("PX {x} {y} {color:06x}\n");
                read_commands += &format!("PX {x} {y}\n");

                color += 1; // Use another color for the next testcase
                combined_commands += &format!("PX {x} {y} {color:06x}\nPX {x} {y}\n");
                combined_commands_expected += &format!("PX {x} {y} {color:06x}\n");

                color += 1;
            }
        }

        // Color the pixels
        let mut stream = MockTcpStream::from_input(&fill_commands);
        network::handle_connection(&mut stream, ip, Arc::clone(&fb), Arc::clone(&statistics));
        assert_eq!("", stream.get_output());

        // Read the pixels again
        let mut stream = MockTcpStream::from_input(&read_commands);
        network::handle_connection(&mut stream, ip, Arc::clone(&fb), Arc::clone(&statistics));
        assert_eq!(fill_commands, stream.get_output());

        // We can also do coloring and reading in a single connection
        let mut stream = MockTcpStream::from_input(&combined_commands);
        network::handle_connection(&mut stream, ip, Arc::clone(&fb), Arc::clone(&statistics));
        assert_eq!(combined_commands_expected, stream.get_output());
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
