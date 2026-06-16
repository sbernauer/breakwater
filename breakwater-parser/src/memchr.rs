use std::sync::Arc;

use crate::{FrameBuffer, Parser};

pub struct MemchrParser<FB: FrameBuffer> {
    fb: Arc<FB>,
}

impl<FB: FrameBuffer> MemchrParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self { fb }
    }
}

impl<FB: FrameBuffer> Parser for MemchrParser<FB> {
    fn parse(&mut self, buffer: &[u8], _response: &mut Vec<u8>) -> usize {
        // As this is a potentially(?) expensive operation we only call it one in this parsing loop
        // All the pixels likely where in the same TCP packets (+- 1/2 or so) it doesn't matter after all
        // Encode the timestamp exactly once here, not per pixel: it's constant for the whole parse
        // call, so computing it per write would just waste throughput on the hot path.
        #[cfg(feature = "time-tracking")]
        let current_ts = self.fb.current_ts();

        let mut last_char_after_newline = 0;
        for newline in memchr::memchr_iter(b'\n', buffer) {
            // TODO Use get_unchecked everywhere
            let line = &buffer[last_char_after_newline..newline.saturating_sub(1)];
            last_char_after_newline = newline + 1;

            assert!(
                !line.is_empty(),
                "Line is empty, we probably should handle this"
            );

            let mut spaces = memchr::memchr_iter(b' ', line);
            let Some(first_space) = spaces.next() else {
                continue;
            };

            if &line[0..first_space] == b"PX" {
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

                self.fb.set(
                    x as usize,
                    y as usize,
                    rgba,
                    #[cfg(feature = "time-tracking")]
                    current_ts,
                );
            }
        }

        last_char_after_newline.saturating_sub(1)
    }

    fn parser_lookahead(&self) -> usize {
        0
    }
}
