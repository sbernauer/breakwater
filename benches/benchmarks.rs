use breakwater::{
    framebuffer::FrameBuffer,
    parser::{from_hex_char_lookup, from_hex_char_map, parse_pixelflut_commands, ParserState},
    test::helpers::{get_commands_to_draw_rect, DevNullTcpStream},
};
use criterion::{
    BenchmarkId, Criterion, {criterion_group, criterion_main},
};
use std::{sync::Arc, time::Duration};

const FRAMEBUFFER_WIDTH: usize = 1920;
const FRAMEBUFFER_HEIGHT: usize = 1080;

async fn invoke_parse_pixelflut_commands(
    input: &[u8],
    fb: &Arc<FrameBuffer>,
    parser_state: ParserState,
) {
    let mut stream = DevNullTcpStream::default();
    parse_pixelflut_commands(input, fb, &mut stream, parser_state).await;
}

#[allow(unused)] // Benchmarks are commented out by default
fn invoke_from_hex_char_map() -> u8 {
    // So that we actually compute something
    let mut result = 0;
    for char in b'0'..=b'9' {
        result |= from_hex_char_map(char);
    }
    for char in b'a'..=b'f' {
        result |= from_hex_char_map(char);
    }
    for char in b'A'..=b'F' {
        result |= from_hex_char_map(char);
    }
    result |= from_hex_char_map(b'\n');
    result |= from_hex_char_map(b' ');
    result |= from_hex_char_map(b';');
    result |= from_hex_char_map(b'%');
    result
}

#[allow(unused)] // Benchmarks are commented out by default
fn invoke_from_hex_char_lookup() -> u8 {
    // So that we actually compute something
    let mut result = 0;
    for char in b'0'..=b'9' {
        result |= from_hex_char_lookup(char);
    }
    for char in b'a'..=b'f' {
        result |= from_hex_char_lookup(char);
    }
    for char in b'A'..=b'F' {
        result |= from_hex_char_lookup(char);
    }
    result |= from_hex_char_lookup(b'\n');
    result |= from_hex_char_lookup(b' ');
    result |= from_hex_char_lookup(b';');
    result |= from_hex_char_lookup(b'%');
    result
}

fn from_elem(c: &mut Criterion) {
    let draw_commands = get_commands_to_draw_rect(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT, 0x123456);
    let draw_commands = draw_commands.as_bytes();

    c.bench_with_input(
        BenchmarkId::new(
            "parse_draw_commands",
            format!("{FRAMEBUFFER_WIDTH} x {FRAMEBUFFER_HEIGHT}"),
        ),
        &draw_commands,
        |b, input| {
            let fb = Arc::new(FrameBuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT));
            let parser_state = ParserState::default();
            b.to_async(tokio::runtime::Runtime::new().unwrap())
                .iter(|| invoke_parse_pixelflut_commands(input, &fb, parser_state.clone()));
        },
    );

    // let read_commands = get_commands_to_read_rect(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT);
    // let read_commands = read_commands.as_bytes();

    // c.bench_with_input(
    //     BenchmarkId::new(
    //         "parse_read_commands",
    //         format!("{FRAMEBUFFER_WIDTH} x {FRAMEBUFFER_HEIGHT}"),
    //     ),
    //     &read_commands,
    //     |b, input| {
    //         let fb = Arc::new(FrameBuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT));
    //         let parser_state = ParserState::default();
    //         b.to_async(tokio::runtime::Runtime::new().unwrap())
    //             .iter(|| invoke_parse_pixelflut_commands(input, &fb, parser_state.clone()));
    //     },
    // );

    // c.bench_function("from_hex_char_map", |b: &mut criterion::Bencher| {
    //     b.iter(invoke_from_hex_char_map)
    // });
    // c.bench_function("from_hex_char_lookup", |b: &mut criterion::Bencher| {
    //     b.iter(invoke_from_hex_char_lookup)
    // });
}

criterion_group!(
    name = benches;
    config = Criterion::default().warm_up_time(Duration::from_secs(10)).measurement_time(Duration::from_secs(30));
    targets = from_elem
);
criterion_main!(benches);

// Performance numbers

// Starting point (while loop)                                      17.967 ms (does not count)   26.289 ms   26.725 ms
// Loop {} instead of while                                         19.118 ms (does not count)   26.563 ms   28.125 ms
// => Change not worth it. Use while loop for better readability

// Starting point (checking for command by indexing [u8] buffer)    27.332 ms   27.175 ms   27.848 ms
// Check for command by reading u32                                 25.590 ms   25.327 ms   23.174 ms
// => Accepted the change :) So far we have only read changed the parsing for "PX " logic, lets also change the other parsing logics
// Check for command by reading u32 everywhere                      24.465 ms   23.435 ms   22.087 ms
