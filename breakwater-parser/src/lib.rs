// Needed for simple implementation
#![feature(portable_simd)]

use enum_dispatch::enum_dispatch;
use snafu::Snafu;
use std::sync::mpsc::Sender;

#[cfg(target_arch = "x86_64")]
pub mod assembler;
pub mod memchr;
pub mod original;
pub mod refactored;

#[derive(Debug, Snafu)]
pub enum ParserError {
    #[snafu(display("Failed to write to TCP socket"))]
    WriteToTcpSocket {
        source: std::sync::mpsc::SendError<Box<[u8]>>,
    },
}

#[enum_dispatch(ParserImplementation)]
// According to https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits.html
#[trait_variant::make(SendParser: Send)]
pub trait Parser {
    fn parse(
        &mut self,
        buffer: &[u8],
        message_sender: &Sender<Box<[u8]>>,
    ) -> Result<usize, ParserError>;

    // Sadly this cant be const (yet?) (https://github.com/rust-lang/rust/issues/71971 and https://github.com/rust-lang/rfcs/pull/2632)
    fn parser_lookahead(&self) -> usize;
}

#[enum_dispatch]
pub enum ParserImplementation {
    Original(original::OriginalParser),
    Refactored(refactored::RefactoredParser),
    Naive(memchr::MemchrParser),
    #[cfg(target_arch = "x86_64")]
    Assembler(assembler::AssemblerParser),
}
