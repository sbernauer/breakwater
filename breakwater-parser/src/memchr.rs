use std::sync::Arc;

use breakwater_core::framebuffer::FrameBuffer;
use tokio::io::AsyncWriteExt;

use crate::{Parser, ParserError};

pub struct MemchrParser {
    fb: Arc<FrameBuffer>,
}

impl MemchrParser {
    pub fn new(fb: Arc<FrameBuffer>) -> Self {
        Self { fb }
    }
}

impl Parser for MemchrParser {
    async fn parse(
        &mut self,
        buffer: &[u8],
        _stream: impl AsyncWriteExt + Send + Unpin,
    ) -> Result<usize, ParserError> {
        let mut last_char_after_newline = 0;
        for newline in memchr::memchr_iter(b'\n', buffer) {
            // TODO Use get_unchecked everywhere
            let line = &buffer[last_char_after_newline..newline.saturating_sub(1)];
            last_char_after_newline = newline + 1;

            if line.is_empty() {
                panic!("Line is empty, we probably should handle this");
            }

            let mut spaces = memchr::memchr_iter(b' ', line);
            let Some(first_space) = spaces.next() else {
                continue;
            };

            match &line[0..first_space] {
                b"PX" => {
                    let Some(second_space) = spaces.next() else {
                        continue;
                    };
                    let Some(third_space) = spaces.next() else {
                        continue;
                    };
                    let Some(fourth_space) = spaces.next() else {
                        continue;
                    };
                    let x: u16 = std::str::from_utf8(&line[first_space + 1..second_space])
                        .expect("Not utf-8")
                        .parse()
                        .expect("x was not a number");
                    let y: u16 = std::str::from_utf8(&line[second_space + 1..third_space])
                        .expect("Not utf-8")
                        .parse()
                        .expect("y was not a number");
                    let rgba: u32 = std::str::from_utf8(&line[third_space + 1..fourth_space])
                        .expect("Not utf-8")
                        .parse()
                        .expect("rgba was not a number");

                    self.fb.set(x as usize, y as usize, rgba);
                }
                _ => {
                    continue;
                }
            }
        }

        Ok(last_char_after_newline.saturating_sub(1))
    }

    fn parser_lookahead(&self) -> usize {
        0
    }
}
