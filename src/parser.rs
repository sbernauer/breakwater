use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use crate::framebuffer::FrameBuffer;

pub const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command
pub const HELP_TEXT: &[u8] = "\
Pixelflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color
PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb (alpha channel is ignored for now)
PX x y: Get the color value of the pixel (x,y)
SIZE: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
OFFSET x y: Apply offset (x,y) to all further pixel draws on this connection
".as_bytes();

#[derive(Clone, Default, Debug)]
pub struct ParserState {
    connection_x_offset: usize,
    connection_y_offset: usize,
    last_byte_parsed: usize,
}

impl ParserState {
    pub fn last_byte_parsed(&self) -> usize {
        self.last_byte_parsed
    }
}

/// Returns the offset (think of index in [u8]) of the last bytes of the last fully parsed command.
///
/// TODO: Implement support for 16K (15360 × 8640).
/// Currently the parser only can read up to 4 digits of x or y coordinates.
/// If you buy me a big enough screen I will kindly implement this feature.
pub async fn parse_pixelflut_commands(
    buffer: &[u8],
    fb: &Arc<FrameBuffer>,
    mut stream: impl AsyncWriteExt + Unpin,
    // We don't pass this as mutual reference but instead hand it around - I guess on the stack?
    // I don't know what I'm doing, hoping for best performance anyway ;)
    parser_state: ParserState,
) -> ParserState {
    let mut last_byte_parsed = 0;
    let mut connection_x_offset = parser_state.connection_x_offset;
    let mut connection_y_offset = parser_state.connection_y_offset;

    let mut x: usize;
    let mut y: usize;

    let mut i = 0; // We can't use a for loop here because Rust don't lets use skip characters by incrementing i
    let loop_end = buffer.len().saturating_sub(PARSER_LOOKAHEAD); // Let's extract the .len() call and the subtraction into it's own variable so we only compute it once

    while i < loop_end {
        // Check for buffer[i] = "PX "
        if unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() } & 0x00ff_ffff
            == 0x50582000_u32.swap_bytes()
        {
            i += 3;
            // Parse first x coordinate char
            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                x = (buffer[i] - b'0') as usize;
                i += 1;

                // Parse optional second x coordinate char
                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                    // TODO: Test bitshifts and add instead of multiplication
                    // i = (i << 3) + (i << 1);
                    // i = (i * 8) + (i * 2);
                    // i = 8i + 2i
                    // i = 10i
                    x = 10 * x + (buffer[i] - b'0') as usize;
                    i += 1;

                    // Parse optional third x coordinate char
                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                        x = 10 * x + (buffer[i] - b'0') as usize;
                        i += 1;

                        // Parse optional forth x coordinate char
                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                            x = 10 * x + (buffer[i] - b'0') as usize;
                            i += 1;
                        }
                    }
                }

                // Separator between x and y
                if buffer[i] == b' ' {
                    i += 1;

                    // Parse first y coordinate char
                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                        y = (buffer[i] - b'0') as usize;
                        i += 1;

                        // Parse optional second y coordinate char
                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                            y = 10 * y + (buffer[i] - b'0') as usize;
                            i += 1;

                            // Parse optional third y coordinate char
                            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                y = 10 * y + (buffer[i] - b'0') as usize;
                                i += 1;

                                // Parse optional forth y coordinate char
                                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                    y = 10 * y + (buffer[i] - b'0') as usize;
                                    i += 1;
                                }
                            }
                        }

                        x += connection_x_offset;
                        y += connection_y_offset;

                        // Separator between coordinates and color
                        if buffer[i] == b' ' {
                            i += 1;

                            // TODO: Determine what clients use more: RGB or RGBA.
                            // If RGBA is used more often move the RGB code below the RGBA code

                            // Must be followed by 6 bytes RGB and newline or ...
                            if buffer[i + 6] == b'\n' {
                                last_byte_parsed = i + 6;
                                i += 7; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                // 30% slower (38,334 ms vs 29,385 ms)
                                // let str = unsafe {
                                //     std::str::from_utf8_unchecked(&buffer[i - 7..i - 2])
                                // };
                                // let rgba = u32::from_str_radix(str, 16).unwrap();

                                let rgba: u32 =
                                    (ASCII_HEXADECIMAL_VALUES[buffer[i - 3] as usize] as u32) << 20
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 2] as usize] as u32)
                                            << 16
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 5] as usize] as u32)
                                            << 12
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 4] as usize] as u32)
                                            << 8
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 7] as usize] as u32)
                                            << 4
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 6] as usize] as u32);

                                fb.set(x, y, rgba);
                                if cfg!(feature = "count_pixels") {
                                    // statistics.inc_pixels(ip);
                                }
                                continue;
                            }

                            // ... or must be followed by 8 bytes RGBA and newline
                            if buffer[i + 8] == b'\n' {
                                last_byte_parsed = i + 8;
                                i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                let rgba: u32 =
                                    (ASCII_HEXADECIMAL_VALUES[buffer[i - 5] as usize] as u32) << 20
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 4] as usize] as u32)
                                            << 16
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 7] as usize] as u32)
                                            << 12
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 6] as usize] as u32)
                                            << 8
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 9] as usize] as u32)
                                            << 4
                                        | (ASCII_HEXADECIMAL_VALUES[buffer[i - 8] as usize] as u32);

                                fb.set(x, y, rgba);
                                if cfg!(feature = "count_pixels") {
                                    // statistics.inc_pixels(ip);
                                }

                                continue;
                            }
                        }

                        // End of command to read Pixel value
                        if buffer[i] == b'\n' {
                            last_byte_parsed = i;
                            i += 1;
                            if let Some(rgb) = fb.get(x, y) {
                                match stream
                                    .write_all(
                                        format!(
                                            "PX {} {} {:06x}\n",
                                            // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                                            x - connection_x_offset,
                                            y - connection_y_offset,
                                            rgb.to_be() >> 8
                                        )
                                        .as_bytes(),
                                    )
                                    .await
                                {
                                    Ok(_) => (),
                                    Err(_) => continue,
                                }
                            }
                            continue;
                        }
                    }
                }
            }
        // Check for buffer[i] = "SIZE"
        } else if unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() }
            == 0x53495a45_u32.swap_bytes()
        {
            i += 4;
            last_byte_parsed = i - 1;

            stream
                .write_all(format!("SIZE {} {}\n", fb.get_width(), fb.get_height()).as_bytes())
                .await
                .expect("Failed to write bytes to tcp socket");
            continue;
        // Check for buffer[i] = "HELP"
        } else if unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() }
            == 0x48454c50_u32.swap_bytes()
        {
            i += 4;
            last_byte_parsed = i - 1;

            stream
                .write_all(HELP_TEXT)
                .await
                .expect("Failed to write bytes to tcp socket");
            continue;
        // Check for buffer[i] = "OFFSET "
        } else if unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() }
            & 0x0000_ffff_ffff_ffff
            == 0x4f464653455420_u64.swap_bytes()
        {
            i += 7;
            // Parse first x coordinate char
            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                x = (buffer[i] - b'0') as usize;
                i += 1;

                // Parse optional second x coordinate char
                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                    x = 10 * x + (buffer[i] - b'0') as usize;
                    i += 1;

                    // Parse optional third x coordinate char
                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                        x = 10 * x + (buffer[i] - b'0') as usize;
                        i += 1;

                        // Parse optional forth x coordinate char
                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                            x = 10 * x + (buffer[i] - b'0') as usize;
                            i += 1;
                        }
                    }
                }

                // Separator between x and y
                if buffer[i] == b' ' {
                    i += 1;

                    // Parse first y coordinate char
                    if buffer[i] >= b'0' && buffer[i] <= b'9' {
                        y = (buffer[i] - b'0') as usize;
                        i += 1;

                        // Parse optional second y coordinate char
                        if buffer[i] >= b'0' && buffer[i] <= b'9' {
                            y = 10 * y + (buffer[i] - b'0') as usize;
                            i += 1;

                            // Parse optional third y coordinate char
                            if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                y = 10 * y + (buffer[i] - b'0') as usize;
                                i += 1;

                                // Parse optional forth y coordinate char
                                if buffer[i] >= b'0' && buffer[i] <= b'9' {
                                    y = 10 * y + (buffer[i] - b'0') as usize;
                                    i += 1;
                                }
                            }
                        }

                        // End of command to set offset
                        if buffer[i] == b'\n' {
                            last_byte_parsed = i;
                            connection_x_offset = x;
                            connection_y_offset = y;
                            continue;
                        }
                    }
                }
            }
        }

        i += 1;
    }

    ParserState {
        connection_x_offset,
        connection_y_offset,
        last_byte_parsed,
    }
}

#[inline(always)]
pub fn from_hex_char_map(char: u8) -> u8 {
    match char {
        b'0'..=b'9' => char - b'0',
        b'a'..=b'f' => char - b'a' + 10,
        b'A'..=b'F' => char - b'A' + 10,
        _ => 0,
    }
}

// fn main() {
// let numbers = (0..=255)
//     .map(|char| match char {
//         b'0'..=b'9' => char - b'0',
//         b'a'..=b'f' => char - b'a' + 10,
//         b'A'..=b'F' => char - b'A' + 10,
//         _ => 0,
//     })
//     .map(|number| number.to_string())
//     .collect::<Vec<String>>();
// println!("{}", numbers.join(", "));
// }
const ASCII_HEXADECIMAL_VALUES: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0,
    0, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0,
];

#[inline(always)]
pub fn from_hex_char_lookup(char: u8) -> u8 {
    ASCII_HEXADECIMAL_VALUES[char as usize]
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_from_hex_char() {
        for c in 0..=255 {
            assert_eq!(from_hex_char_map(c), from_hex_char_map(c));
        }
    }
}
