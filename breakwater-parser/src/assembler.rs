use std::arch::asm;

use tokio::io::AsyncWriteExt;

use crate::{Parser, ParserError};

const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command

#[derive(Default)]
pub struct AssemblerParser {}

impl Parser for AssemblerParser {
    async fn parse(
        &mut self,
        buffer: &[u8],
        _stream: impl AsyncWriteExt + Send + Unpin,
    ) -> Result<usize, ParserError> {
        let mut last_byte_parsed = 0;

        // This loop does nothing and should be seen as a placeholder
        unsafe {
            asm!(
                "mov {i}, {buffer_start}",
                "2:",
                "inc {last_byte_parsed}",
                "inc {i}",
                "cmp {i}, {buffer_end}",
                "jl 2b",
                buffer_start = in(reg) buffer.as_ptr(),
                buffer_end = in(reg) buffer.as_ptr().add(buffer.len()),
                last_byte_parsed = inout(reg) last_byte_parsed,
                i = out(reg) _,
            )
        }

        Ok(last_byte_parsed)
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
    }
}
