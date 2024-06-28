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
    // We keep the framebuffer so small, so that we can easily test all pixels in a test run
    Arc::new(FrameBuffer::new(640, 480))
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
#[case("SIZE", "SIZE 640 480\n")]
#[case("SIZE\n", "SIZE 640 480\n")]
#[case("SIZE\nSIZE\n", "SIZE 640 480\nSIZE 640 480\n")]
#[case("SIZE", "SIZE 640 480\n")]
#[case("HELP", std::str::from_utf8(HELP_TEXT).unwrap())]
#[case("HELP\n", std::str::from_utf8(HELP_TEXT).unwrap())]
#[case("bla bla bla\nSIZE\nblub\nbla", "SIZE 640 480\n")]
#[tokio::test]
async fn test_correct_responses_to_general_commands(#[case] input: &str, #[case] expected: &str) {
    assert_returns(input.as_bytes(), expected).await;
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
async fn test_setting_pixel(#[case] input: &str, #[case] expected: &str) {
    assert_returns(input.as_bytes(), expected).await;
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
    let mut stream = MockTcpStream::from_string(input);
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

    for x in 0..fb.get_width() {
        for y in 0..fb.get_height() {
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
    let mut stream = MockTcpStream::from_string(&fill_commands);
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
    let mut stream = MockTcpStream::from_string(&read_commands);
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
    let mut stream = MockTcpStream::from_string(&combined_commands);
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
    let mut stream = MockTcpStream::from_string(&read_other_pixels_commands);
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

#[cfg(feature = "binary-set-single-pixel")]
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
    let mut stream = MockTcpStream::from_string(input);
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

#[cfg(feature = "binary-sync-pixels")]
#[tokio::test]
async fn test_binary_sync_pixels() {
    // Test byte conversion works
    assert_returns("PX 0 0 42\nPX 0 0\n".as_bytes(), "PX 0 0 424242\n").await;

    // Don't set any pixels
    let mut input = Vec::new();
    input.extend("PXMULTI".as_bytes());
    input.extend([
        0, 0, /* startX */
        0, 0, /* startY */
        0, 0, 0, 0, /* length */
    ]);
    input.extend("PX 0 0\n".as_bytes());
    assert_returns(&input, "PX 0 0 000000\n").await;

    // Set first 10 pixels
    let mut input = Vec::new();
    input.extend("PXMULTI".as_bytes());
    input.extend(0_u16.to_le_bytes()); // x
    input.extend(0_u16.to_le_bytes()); // y
    input.extend(10_u32.to_le_bytes()); // length
    for pixel in 0..10_u32 {
        // Some alpha stuff going on (which I don't fully understand)
        input.extend((pixel << 8).to_be_bytes());
    }
    input.extend(
        "PX 0 0\nPX 1 0\nPX 2 0\nPX 3 0\nPX 4 0\nPX 5 0\nPX 6 0\nPX 7 0\nPX 8 0\nPX 9 0\n"
            .as_bytes(),
    );
    assert_returns(&input, "PX 0 0 000000\nPX 1 0 000001\nPX 2 0 000002\nPX 3 0 000003\nPX 4 0 000004\nPX 5 0 000005\nPX 6 0 000006\nPX 7 0 000007\nPX 8 0 000008\nPX 9 0 000009\n").await;
}

#[cfg(feature = "binary-sync-pixels")]
#[rstest]
#[tokio::test]
/// Try painting the very last pixel of the screen. There is only space for a single pixel left.
async fn test_binary_sync_pixels_last_pixel(fb: Arc<FrameBuffer>) {
    let mut input = Vec::new();
    let x = fb.get_width() as u16 - 1;
    let y = fb.get_height() as u16 - 1;
    input.extend("PXMULTI".as_bytes());
    input.extend(x.to_le_bytes()); // x
    input.extend(y.to_le_bytes()); // y
    input.extend(1_u32.to_le_bytes()); // length
    input.extend(0x12345678_u32.to_be_bytes());

    input.extend(format!("PX 0 0\nPX {} {y}\nPX {x} {y}\n", x - 1).as_bytes());
    assert_returns(
        &input,
        &format!(
            "PX 0 0 000000\nPX {} {y} 000000\nPX {x} {y} 123456\n",
            x - 1
        ),
    )
    .await;
}

#[cfg(feature = "binary-sync-pixels")]
#[rstest]
#[tokio::test]
/// Try painting some pixels in the middle of the screen
async fn test_binary_sync_pixels_in_the_middle(fb: Arc<FrameBuffer>) {
    let mut input = Vec::new();
    let mut expected = String::new();

    let x = 42_u16;
    let y = 13_u16;
    let num_pixels = fb.get_width() as u32 + 10;
    input.extend("PXMULTI".as_bytes());
    input.extend(x.to_le_bytes()); // x
    input.extend(y.to_le_bytes()); // y
    input.extend(num_pixels.to_le_bytes()); // length

    for rgba in 0..num_pixels {
        input.extend((rgba << 8).to_be_bytes());
    }

    let mut rgba = 0_u32;
    for x in 42..fb.get_width() {
        input.extend(format!("PX {x} 13\n").as_bytes());
        expected += &format!("PX {x} 13 {rgba:06x}\n");
        rgba += 1;
    }

    for x in 0..52 {
        input.extend(format!("PX {x} 14\n").as_bytes());
        expected += &format!("PX {x} 14 {rgba:06x}\n");
        rgba += 1;
    }

    input.extend(format!("PX 52 14\n").as_bytes());
    expected += &format!("PX 52 14 000000\n");

    assert_returns(&input, &expected).await;
}

#[cfg(feature = "binary-sync-pixels")]
#[rstest]
#[tokio::test]
/// Try painting too much pixels, so it overflows the framebuffer.
async fn test_binary_sync_pixels_exceeding_screen(fb: Arc<FrameBuffer>) {
    let mut input = Vec::new();
    let x = fb.get_width() as u16 - 1;
    let y = fb.get_height() as u16 - 1;
    input.extend("PXMULTI".as_bytes());
    input.extend(x.to_le_bytes()); // x
    input.extend(y.to_le_bytes()); // y
    input.extend(2_u32.to_le_bytes()); // length
    input.extend(0x12345678_u32.to_be_bytes());
    input.extend(0x87654321_u32.to_be_bytes());

    input.extend(format!("PX {x} {y}\n").as_bytes());
    // As we exceeded the screen nothing should have been set
    assert_returns(&input, &format!("PX {x} {y} 000000\n")).await;
}

#[cfg(feature = "binary-sync-pixels")]
#[rstest]
#[tokio::test]
/// Try painting more pixels that fit in the buffer. This checks if the parse correctly keeps track of the command
/// across multiple parse calls as the pixel screen send is bigger than the buffer.
async fn test_binary_sync_pixels_larger_than_buffer(fb: Arc<FrameBuffer>) {
    let fb = Arc::new(FrameBuffer::new(50, 30));

    let num_pixels = (fb.get_width() * fb.get_height()) as u32;
    let pixel_bytes =  num_pixels * 4 /* bytes per pixel */;
    // assert!(
    //     pixel_bytes > DEFAULT_NETWORK_BUFFER_SIZE as u32 * 3,
    //     "The number of bytes we send must be bigger than the network buffer size so that we test the wrapping. \
    //     We actually pick a bit more, just to be safe and do a few cycles. Additionally, in tests the number of bytes \
    //     read into the socket is actually around 2074 (and differs each run), so we should be really good here"
    // );

    let mut input = Vec::new();
    let mut expected = String::new();
    input.extend("PXMULTI".as_bytes());
    input.extend(0_u16.to_le_bytes()); // x
    input.extend(0_u16.to_le_bytes()); // y
    input.extend(num_pixels.to_le_bytes()); // length

    for rgba in 0..num_pixels {
        // input.extend((rgba << 8).to_be_bytes());
        input.extend((0xffffffff_u32 << 8).to_be_bytes());
    }

    let mut rgba = 0_u32;
    // Watch out, we first iterate over y, than x
    for y in 0..fb.get_height() {
        for x in 0..fb.get_width() {
            // rgba = 0xdeadbeef_u32;
            rgba = 0xffffffff;
            input.extend(format!("PX {x} {y}\n").as_bytes());
            expected += &format!("PX {x} {y} {rgba:06x}\n");
            // rgba += 1;
        }
    }

    let mut stream = MockTcpStream::from_bytes(input.to_owned());
    handle_connection(
        &mut stream,
        ip(),
        fb,
        statistics_channel().0,
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(expected, stream.get_output());
}

async fn assert_returns(input: &[u8], expected: &str) {
    let mut stream = MockTcpStream::from_bytes(input.to_owned());
    handle_connection(
        &mut stream,
        ip(),
        fb(),
        statistics_channel().0,
        DEFAULT_NETWORK_BUFFER_SIZE,
        page_size::get(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(expected, stream.get_output());
}
