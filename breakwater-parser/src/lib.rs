// Needed for simple implementation
#![feature(portable_simd)]

use enum_dispatch::enum_dispatch;
use snafu::Snafu;
use tokio::io::AsyncWriteExt;

#[cfg(target_arch = "x86_64")]
pub mod assembler;
pub mod memchr;
pub mod original;
pub mod refactored;

#[derive(Debug, Snafu)]
pub enum ParserError {
    #[snafu(display("Failed to write to TCP socket"))]
    WriteToTcpSocket { source: std::io::Error },
}

#[enum_dispatch(ParserImplementation)]
// According to https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits.html
#[trait_variant::make(SendParser: Send)]
pub trait Parser {
    async fn parse(
        &mut self,
        buffer: &[u8],
        stream: impl AsyncWriteExt + Send + Unpin,
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
