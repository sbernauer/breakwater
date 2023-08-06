use breakwater::{
    framebuffer::FrameBuffer,
    parser::{parse_pixelflut_commands, ParserState},
    test::helpers::DevNullTcpStream,
};
use criterion::{
    BenchmarkId, Criterion, {criterion_group, criterion_main},
};
use pixelbomber::image_handler;
use pixelbomber::image_handler::ImageConfig;
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

fn benchmark_settings(c: &mut Criterion, name: &str, file: &str, config: ImageConfig) {
    let mut commands = image_handler::load(vec![file], &config);
    let command = commands.pop().unwrap();
    c.bench_with_input(
        BenchmarkId::new(name, format!("{FRAMEBUFFER_WIDTH} x {FRAMEBUFFER_HEIGHT}")),
        &command,
        |b, input| {
            let fb = Arc::new(FrameBuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT));
            let parser_state = ParserState::default();
            b.to_async(tokio::runtime::Runtime::new().unwrap())
                .iter(|| invoke_parse_pixelflut_commands(input, &fb, parser_state.clone()));
        },
    );
}

fn from_elem(c: &mut Criterion) {
    benchmark_settings(
        c,
        "parse_draw_commands_ordered",
        "benches/non-transparent.png",
        image_handler::ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(false)
            .build(),
    );

    benchmark_settings(
        c,
        "parse_draw_commands_shuffled",
        "benches/non-transparent.png",
        image_handler::ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(true)
            .build(),
    );

    benchmark_settings(
        c,
        "parse_mixed_draw_commands",
        "benches/mixed.png",
        image_handler::ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(false)
            .gray_usage(true)
            .build(),
    );

    benchmark_settings(
        c,
        "parse_draw_commands_with_offset",
        "benches/non-transparent.png",
        image_handler::ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(false)
            .offset_usage(true)
            .build(),
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
