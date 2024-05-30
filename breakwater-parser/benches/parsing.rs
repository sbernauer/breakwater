use std::sync::mpsc::channel;
use std::{sync::Arc, time::Duration};

use breakwater_core::framebuffer::FrameBuffer;
#[cfg(target_arch = "x86_64")]
use breakwater_parser::assembler::AssemblerParser;
use breakwater_parser::{
    memchr::MemchrParser, original::OriginalParser, refactored::RefactoredParser, Parser,
    ParserImplementation,
};
use criterion::{criterion_group, criterion_main, Criterion};
use pixelbomber::image_handler::{self, ImageConfigBuilder};

const FRAMEBUFFER_WIDTH: usize = 1920;
const FRAMEBUFFER_HEIGHT: usize = 1080;

fn compare_implementations(c: &mut Criterion) {
    invoke_benchmark(
        c,
        "parse_draw_commands_ordered",
        "benches/non-transparent.png",
        false,
        false,
        false,
    );
    invoke_benchmark(
        c,
        "parse_draw_commands_unordered",
        "benches/non-transparent.png",
        true,
        false,
        false,
    );
    invoke_benchmark(
        c,
        "parse_draw_commands_with_offset",
        "benches/non-transparent.png",
        true,
        true,
        false,
    );
    invoke_benchmark(
        c,
        "parse_mixed_draw_commands",
        "benches/mixed.png",
        false,
        false,
        true,
    );
}

fn invoke_benchmark(
    c: &mut Criterion,
    bench_name: &str,
    image: &str,
    shuffle: bool,
    use_offset: bool,
    use_gray: bool,
) {
    let commands = image_handler::load(
        vec![image],
        ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(shuffle)
            .offset_usage(use_offset)
            .gray_usage(use_gray)
            .build(),
    )
    .pop()
    .expect("Fail to retrieve Pixelflut commands");

    let mut c_group = c.benchmark_group(bench_name);

    let fb = Arc::new(FrameBuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT));

    #[cfg(target_arch = "x86_64")]
    let parser_names = ["original", "refactored", "memchr", "assembler"];
    #[cfg(not(target_arch = "x86_64"))]
    let parser_names = ["original", "refactored", "memchr"];

    for parse_name in parser_names {
        c_group.bench_with_input(parse_name, &commands, |b, input| {
            b.to_async(tokio::runtime::Runtime::new().expect("Failed to start tokio runtime"))
                .iter(|| async {
                    let (message_sender, _) = channel();
                    let mut parser = match parse_name {
                        "original" => {
                            ParserImplementation::Original(OriginalParser::new(fb.clone()))
                        }
                        "refactored" => {
                            ParserImplementation::Refactored(RefactoredParser::new(fb.clone()))
                        }
                        "memchr" => ParserImplementation::Naive(MemchrParser::new(fb.clone())),
                        #[cfg(target_arch = "x86_64")]
                        "assembler" => ParserImplementation::Assembler(AssemblerParser::default()),
                        _ => panic!("Parser implementation {parse_name} not known"),
                    };

                    parser
                        .parse(input, &message_sender)
                        .expect("Failed to parse commands");
                });
        });
    }
}

criterion_group!(
    name = parsing;
    config = Criterion::default().warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
    targets = compare_implementations
);
criterion_main!(parsing);
