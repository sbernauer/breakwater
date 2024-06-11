#![allow(clippy::octal_escapes)]

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use breakwater_core::{framebuffer::FrameBuffer, test_helpers::MockTcpStream, HELP_TEXT};
use rstest::{fixture, rstest};
use tokio::sync::mpsc;

use crate::{
    cli_args::DEFAULT_NETWORK_BUFFER_SIZE, server::handle_connection, statistics::StatisticsEvent,
};

#[fixture]
fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
}

#[fixture]
fn fb() -> Arc<FrameBuffer> {
    Arc::new(FrameBuffer::new(1920, 1080))
}

#[fixture]
fn statistics_channel() -> (
    mpsc::Sender<StatisticsEvent>,
    mpsc::Receiver<StatisticsEvent>,
) {
    mpsc::channel(10000)
}

#[rstest]
#[timeout(std::time::Duration::from_secs(1))]
#[case("", "")]
#[case("\n", "")]
#[case("not a pixelflut command", "")]
#[case("not a pixelflut command with newline\n", "")]
#[case("SIZE", "SIZE 1920 1080\n")]
#[case("SIZE\n", "SIZE 1920 1080\n")]
#[case("SIZE\nSIZE\n", "SIZE 1920 1080\nSIZE 1920 1080\n")]
#[case("SIZE", "SIZE 1920 1080\n")]
#[case("HELP", std::str::from_utf8(HELP_TEXT).unwrap())]
#[case("HELP\n", std::str::from_utf8(HELP_TEXT).unwrap())]
#[case("bla bla bla\nSIZE\nblub\nbla", "SIZE 1920 1080\n")]
#[tokio::test]
async fn test_correct_responses_to_general_commands(
    #[case] input: &str,
    #[case] expected: &str,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_channel: (
        mpsc::Sender<StatisticsEvent>,
        mpsc::Receiver<StatisticsEvent>,
    ),
) {
    let mut stream = MockTcpStream::from_input(input);
    handle_connection(
        &mut stream,
        ip,
        fb,
        statistics_channel.0,
        page_size::get(),
        DEFAULT_NETWORK_BUFFER_SIZE,
        None,
    )
    .await
    .unwrap();

    assert_eq!(expected, stream.get_output());
}

#[rstest]
// Without alpha
#[case("PX 0 0 ffffff\nPX 0 0\n", "PX 0 0 ffffff\n")]
#[case("PX 0 0 abcdef\nPX 0 0\n", "PX 0 0 abcdef\n")]
#[case("PX 0 42 abcdef\nPX 0 42\n", "PX 0 42 abcdef\n")]
#[case("PX 42 0 abcdef\nPX 42 0\n", "PX 42 0 abcdef\n")]
// With alpha
#[case("PX 0 0 ffffff00\nPX 0 0\n", if cfg!(feature = "alpha") {"PX 0 0 000000\n"} else {"PX 0 0 ffffff\n"})]
#[case("PX 0 0 ffffffff\nPX 0 0\n", "PX 0 0 ffffff\n")]
#[case("PX 0 1 abcdef00\nPX 0 1\n", if cfg!(feature = "alpha") {"PX 0 1 000000\n"} else {"PX 0 1 abcdef\n"})]
#[case("PX 1 0 abcdefff\nPX 1 0\n", "PX 1 0 abcdef\n")]
#[case("PX 0 0 ffffff88\nPX 0 0\n", if cfg!(feature = "alpha") {"PX 0 0 888888\n"} else {"PX 0 0 ffffff\n"})]
#[case("PX 0 0 ffffff11\nPX 0 0\n", if cfg!(feature = "alpha") {"PX 0 0 111111\n"} else {"PX 0 0 ffffff\n"})]
#[case("PX 0 0 abcdef80\nPX 0 0\n", if cfg!(feature = "alpha") {"PX 0 0 556677\n"} else {"PX 0 0 abcdef\n"})]
// 0xab = 171, 0x88 = 136
// (171 * 136) / 255 = 91 = 0x5b
#[case("PX 0 0 abcdef88\nPX 0 0\n", if cfg!(feature = "alpha") {"PX 0 0 5b6d7f\n"} else {"PX 0 0 abcdef\n"})]
// Short commands
#[case("PX 0 0 00\nPX 0 0\n", "PX 0 0 000000\n")]
#[case("PX 0 0 ff\nPX 0 0\n", "PX 0 0 ffffff\n")]
#[case("PX 0 1 12\nPX 0 1\n", "PX 0 1 121212\n")]
#[case("PX 0 1 34\nPX 0 1\n", "PX 0 1 343434\n")]
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
    statistics_channel: (
        mpsc::Sender<StatisticsEvent>,
        mpsc::Receiver<StatisticsEvent>,
    ),
) {
    let mut stream = MockTcpStream::from_input(input);
    handle_connection(
        &mut stream,
        ip,
        fb,
        statistics_channel.0,
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(expected, stream.get_output());
}

#[cfg(feature = "binary-commands")]
#[rstest]
// No newline in between needed
#[case("PB\0\0\0\0\0\0\0\0PX 0 0\n", "PX 0 0 000000\n")]
#[case("PB\0\0\0\01234PX 0 0\n", "PX 0 0 313233\n")]
#[case("PB\0\0\0\0\0\0\0\0PB\0\0\0\01234PX 0 0\n", "PX 0 0 313233\n")]
#[case(
    "PB\0\0\0\0\0\0\0\0PX 0 0\nPB\0\0\0\01234PX 0 0\n",
    "PX 0 0 000000\nPX 0 0 313233\n"
)]
#[case("PB \0*\0____PX 32 42\n", "PX 32 42 5f5f5f\n")]
// Also test that there can be newlines in between
#[case(
    "PB\0\0\0\0\0\0\0\0\nPX 0 0\nPB\0\0\0\01234\n\n\nPX 0 0\n",
    "PX 0 0 000000\nPX 0 0 313233\n"
)]
#[tokio::test]
async fn test_binary_commands(
    #[case] input: &str,
    #[case] expected: &str,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_channel: (
        mpsc::Sender<StatisticsEvent>,
        mpsc::Receiver<StatisticsEvent>,
    ),
) {
    let mut stream = MockTcpStream::from_input(input);
    handle_connection(
        &mut stream,
        ip,
        fb,
        statistics_channel.0,
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(expected, stream.get_output());
}

#[rstest]
#[case("PX 0 0 aaaaaa\n")]
#[case("PX 0 0 aa\n")]
#[tokio::test]
async fn test_safe(
    #[case] input: &str,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_channel: (
        mpsc::Sender<StatisticsEvent>,
        mpsc::Receiver<StatisticsEvent>,
    ),
) {
    let mut stream = MockTcpStream::from_input(input);
    handle_connection(
        &mut stream,
        ip,
        fb.clone(),
        statistics_channel.0,
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();

    // Test if it panics
    assert_eq!(fb.get(0, 0).unwrap() & 0x00ff_ffff, 0xaaaaaa);
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
// Yes, this exceeds the framebuffer size
#[case(10, 10, fb().get_width() - 5, fb().get_height() - 5)]
#[tokio::test]
async fn test_drawing_rect(
    #[case] width: usize,
    #[case] height: usize,
    #[case] offset_x: usize,
    #[case] offset_y: usize,
    ip: IpAddr,
    fb: Arc<FrameBuffer>,
    statistics_channel: (
        mpsc::Sender<StatisticsEvent>,
        mpsc::Receiver<StatisticsEvent>,
    ),
) {
    let mut color: u32 = 0;
    let mut fill_commands = String::new();
    let mut read_commands = String::new();
    let mut combined_commands = String::new();
    let mut combined_commands_expected = String::new();
    let mut read_other_pixels_commands = String::new();
    let mut read_other_pixels_commands_expected = String::new();

    for x in 0..height {
        for y in 0..height {
            // Inside the rect
            if x >= offset_x && x <= offset_x + width && y >= offset_y && y <= offset_y + height {
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
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();
    assert_eq!("", stream.get_output());

    // Read the pixels again
    let mut stream = MockTcpStream::from_input(&read_commands);
    handle_connection(
        &mut stream,
        ip,
        Arc::clone(&fb),
        statistics_channel.0.clone(),
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(fill_commands, stream.get_output());

    // We can also do coloring and reading in a single connection
    let mut stream = MockTcpStream::from_input(&combined_commands);
    handle_connection(
        &mut stream,
        ip,
        Arc::clone(&fb),
        statistics_channel.0.clone(),
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(combined_commands_expected, stream.get_output());

    // Check that nothing else was colored
    let mut stream = MockTcpStream::from_input(&read_other_pixels_commands);
    handle_connection(
        &mut stream,
        ip,
        Arc::clone(&fb),
        statistics_channel.0.clone(),
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(read_other_pixels_commands_expected, stream.get_output());
}
