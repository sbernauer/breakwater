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
        false,
    );
    invoke_benchmark(
        c,
        "parse_binary_draw_commands",
        "benches/non-transparent.png",
        false,
        false,
        false,
        true,
    );
    invoke_benchmark(
        c,
        "parse_draw_commands_unordered",
        "benches/non-transparent.png",
        true,
        false,
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
        false,
    );
    invoke_benchmark(
        c,
        "parse_mixed_draw_commands",
        "benches/mixed.png",
        false,
        false,
        true,
        false,
    );
}

fn invoke_benchmark(
    c: &mut Criterion,
    bench_name: &str,
    image: &str,
    shuffle: bool,
    use_offset: bool,
    use_gray: bool,
    binary_usage: bool,
) {
    let commands = image_handler::load(
        vec![image],
        ImageConfigBuilder::new()
            .width(FRAMEBUFFER_WIDTH as u32)
            .height(FRAMEBUFFER_HEIGHT as u32)
            .shuffle(shuffle)
            .offset_usage(use_offset)
            .gray_usage(use_gray)
            .binary_usage(binary_usage)
            .chunks(1)
            .build(),
    );

    assert_eq!(
        commands.len(),
        1,
        "The returned commands should only return a single image",
    );
    let commands = commands.first().unwrap();

    assert_eq!(
        commands.len(),
        1,
        "The returned commands should only return a single chunk",
    );
    let commands = commands.first().unwrap();

    let mut c_group = c.benchmark_group(bench_name);

    let fb = Arc::new(FrameBuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT));

    let parser_names = vec!["original", "refactored" /*"memchr"*/];

    // #[cfg(target_arch = "x86_64")]
    // parser_names.push("assembler");

    for parse_name in parser_names {
        c_group.bench_with_input(parse_name, &commands, |b, input| {
            b.iter(|| {
                let mut parser = match parse_name {
                    "original" => ParserImplementation::Original(OriginalParser::new(fb.clone())),
                    "refactored" => {
                        ParserImplementation::Refactored(RefactoredParser::new(fb.clone()))
                    }
                    "memchr" => ParserImplementation::Naive(MemchrParser::new(fb.clone())),
                    #[cfg(target_arch = "x86_64")]
                    "assembler" => ParserImplementation::Assembler(AssemblerParser::default()),
                    _ => panic!("Parser implementation {parse_name} not known"),
                };

                parser.parse(input, &mut Vec::new());
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
