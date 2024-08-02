use std::{arch::asm, sync::Arc};

use crate::{FrameBuffer, Parser};

const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command

#[derive(Default)]
pub struct AssemblerParser<FB: FrameBuffer> {
    help_text: &'static [u8],
    alt_help_text: &'static [u8],
    fb: Arc<FB>,
}

impl<FB: FrameBuffer> Parser for AssemblerParser<FB> {
    fn parse(&mut self, buffer: &[u8], _response: &mut Vec<u8>) -> usize {
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

        last_byte_parsed
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
    }
}
