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
    /// Returns the last byte parsed. The next parsing loop will again contain all data that was not parsed.
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize;

    // Sadly this cant be const (yet?) (https://github.com/rust-lang/rust/issues/71971 and https://github.com/rust-lang/rfcs/pull/2632)
    fn parser_lookahead(&self) -> usize;
}

#[enum_dispatch]
pub enum ParserImplementation<FB: FrameBuffer> {
    Original(original::OriginalParser<FB>),
    Refactored(refactored::RefactoredParser<FB>),
    Naive(memchr::MemchrParser<FB>),
    #[cfg(target_arch = "x86_64")]
    Assembler(assembler::AssemblerParser<FB>),
}

pub trait FrameBuffer {
    fn get_width(&self) -> usize;
    fn get_height(&self) -> usize;
    fn get_size(&self) -> usize {
        self.get_width() * self.get_height()
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> Option<u32> {
        if x < self.get_width() && y < self.get_height() {
            Some(unsafe { self.get_unchecked(x, y) })
        } else {
            None
        }
    }
    /// # Safety
    /// make sure x and y are in bounds
    unsafe fn get_unchecked(&self, x: usize, y: usize) -> u32;

    fn set(&self, x: usize, y: usize, rgba: u32);
}
