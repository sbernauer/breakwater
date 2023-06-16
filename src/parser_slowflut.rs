use crate::framebuffer::FrameBuffer;
use const_format::formatcp;
use std::{io::BufRead, sync::Arc};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const TOKEN_LIFETIME: usize = 1000;

pub const PARSER_LOOKAHEAD: usize = 0;
// "PX 1234 1234 rrggbbaa 67e55044-10b1-426f-9247-bb680e5fe0c8\n".len(); // Longest possible command
pub const HELP_TEXT: &[u8] = formatcp!("\
Slowflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP\\n: Show this help
SIZE\\n: Get the size of the drawing surface
TOKEN\\n: Get a token to be used to draw pixels. It will return two values: 1.) The token 2.) The lifetime (how many pixels you can draw with it). You will get a fresh token every time, even wgen you have not fully used your old one
PX x y rrggbb token\\n: Color the pixel (x,y) with the given hexadecimal color rrggbb. You need to pass a TOKEN you have requested using the TOKEN command
PX x y\\n: Get the color value of the pixel (x,y)
"
).as_bytes();

#[derive(Clone, Default, Debug)]
pub struct ParserState {
    token: String,
    token_remaining_draws: usize,
    /// Offset (think of index in [u8]) of the last bytes of the last fully parsed command.
    last_byte_parsed: usize,
}

impl ParserState {
    pub fn last_byte_parsed(&self) -> usize {
        self.last_byte_parsed
    }
}

pub async fn parse_pixelflut_commands(
    buffer: &[u8],
    fb: &Arc<FrameBuffer>,
    mut stream: impl AsyncWriteExt + Unpin,
    parser_state: ParserState,
) -> ParserState {
    dbg!(std::str::from_utf8(buffer).unwrap());
    let mut token = parser_state.token;
    let mut token_remaining_draws = parser_state.token_remaining_draws;

    let last_newline = buffer.iter().rposition(|i| *i == b'\n');
    let last_byte_parsed = match last_newline {
        Some(last_newline) => last_newline,
        None => {
            return ParserState {
                last_byte_parsed: buffer.len().saturating_sub(1),
                token,
                token_remaining_draws,
            }
        }
    };

    // match last_newline {
    //     Some(last_newline) => last_byte_parsed = last_newline,
    //     None => {
    //         // There is not a single newline, let's just skip this
    //         return ParserState {
    //             last_byte_parsed: buffer.len().saturating_sub(1),
    //             token,
    //             token_remaining_draws,
    //         };
    //     }
    // }
    // last_byte_parsed += 0;

    for line in buffer.lines() {
        let line = line.unwrap();
        let mut parts = line.split(' ');
        match parts.next() {
            Some("PX") => {
                let Some(Ok(x)) = parts.next().map(|s| s.parse::<usize>()) else { continue };
                let Some(Ok(y)) = parts.next().map(|s| s.parse::<usize>()) else { continue };
                if let Some(rgb) = parts.next() {
                    if rgb.len() != 6 {
                        continue;
                    }
                    if let Some(mut command_token) = parts.next() {
                        if command_token.len() != 36 {
                            continue;
                        }
                        if token_remaining_draws == 0 {
                            stream
                                    .write_all(
                                        "ERROR: Token has used all available draws (or no token requested)\n"
                                            .as_bytes(),
                                    )
                                    .await
                                    .expect("Failed to write bytes to tcp socket");
                            continue;
                        }
                        command_token = command_token.trim_end_matches('\n');
                        if command_token != token {
                            stream
                                    .write_all(
                                        format!(
                                            "ERROR: Wrong TOKEN, expected {} with {} draws left, got {}\n",
                                            &token,
                                            token_remaining_draws,
                                            command_token
                                        )
                                        .as_bytes(),
                                    )
                                    .await
                                    .expect("Failed to write bytes to tcp socket");
                            continue;
                        }
                        let Ok(rgb) = u32::from_str_radix(&rgb[0..6], 16) else { continue };
                        fb.set(x, y, rgb.to_le());
                        token_remaining_draws -= 1;
                    }
                } else if let Some(rgb) = fb.get(x, y) {
                    stream
                        .write_all(format!("PX {x} {y} {rgb:06x}\n").as_bytes())
                        .await
                        .expect("Failed to write bytes to tcp socket");
                }
            }
            Some("TOKEN") => {
                token = if cfg!(test) {
                    // Hardcoded value to make tests easier
                    "67e55044-10b1-426f-9247-bb680e5fe0c8".to_string()
                } else {
                    Uuid::new_v4().to_string()
                };
                token_remaining_draws = TOKEN_LIFETIME;
                stream
                    .write_all(format!("TOKEN {token} {token_remaining_draws}\n").as_bytes())
                    .await
                    .expect("Failed to write bytes to tcp socket");
            }
            Some("SIZE") => {
                stream
                    .write_all(format!("SIZE {} {}\n", fb.get_width(), fb.get_height()).as_bytes())
                    .await
                    .expect("Failed to write bytes to tcp socket");
            }
            Some("HELP") => {
                stream
                    .write_all(HELP_TEXT)
                    .await
                    .expect("Failed to write bytes to tcp socket");
            }
            _ => {}
        }
    }
    dbg!(
        std::str::from_utf8(&buffer[last_byte_parsed.saturating_sub(5)..last_byte_parsed + 1])
            .unwrap()
    );
    // last_byte_parsed =
    //     last_byte_parsed.saturating_sub(if buffer.last() == Some(&b'\n') { 2 } else { 1 });
    ParserState {
        last_byte_parsed,
        token,
        token_remaining_draws,
    }
}
