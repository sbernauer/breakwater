// Needed for simple implementation
#![feature(portable_simd)]

use enum_dispatch::enum_dispatch;

#[cfg(target_arch = "x86_64")]
pub mod assembler;
pub mod memchr;
pub mod original;
pub mod refactored;

#[enum_dispatch(ParserImplementation)]
pub trait Parser {
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize;

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
