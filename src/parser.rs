use crate::framebuffer::FrameBuffer;
use const_format::formatcp;
use log::{info, warn};
use std::simd::{u32x8, Simd, SimdUint};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command
pub const HELP_TEXT: &[u8] = formatcp!("\
Pixelflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color rrggbb
{}
PX x y gg: Color the pixel (x,y) with the hexadecimal color gggggg. Basically this is the same as the other commands, but is a more efficient way of filling white, black or gray areas
PX x y: Get the color value of the pixel (x,y)
SIZE: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
OFFSET x y: Apply offset (x,y) to all further pixel draws on this connection. This can e.g. be used to pre-calculate an image/animation and simply use the OFFSET command to move it around the screen without the need to re-calculate it
",
if cfg!(feature = "alpha") {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb and a transparency of aa, where ff means draw normally on top of the existing pixel and 00 means fully transparent (no change at all)"
} else {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb. The alpha part is discarded for performance reasons, as breakwater was compiled without the alpha feature"
}
).as_bytes();

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

const fn string_to_number(input: &[u8]) -> u64 {
    (input[7] as u64) << 56
        | (input[6] as u64) << 48
        | (input[5] as u64) << 40
        | (input[4] as u64) << 32
        | (input[3] as u64) << 24
        | (input[2] as u64) << 16
        | (input[1] as u64) << 8
        | (input[0] as u64)
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
        let current_command = unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() };
        if current_command & 0x00ff_ffff == string_to_number(b"PX \0\0\0\0\0") {
            i += 3;
            // Parse x coordinate
            let digits =
                unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() } as usize;

            let mut digit = digits & 0xff;
            if digit >= b'0' as usize && digit <= b'9' as usize {
                x = digit - b'0' as usize;
                i += 1;
                digit = (digits >> 8) & 0xff;
                if digit >= b'0' as usize && digit <= b'9' as usize {
                    x = 10 * x + digit - b'0' as usize;
                    i += 1;
                    digit = (digits >> 16) & 0xff;
                    if digit >= b'0' as usize && digit <= b'9' as usize {
                        x = 10 * x + digit - b'0' as usize;
                        i += 1;
                        digit = (digits >> 24) & 0xff;
                        if digit >= b'0' as usize && digit <= b'9' as usize {
                            x = 10 * x + digit - b'0' as usize;
                            i += 1;
                        }
                    }
                }

                // Separator between x and y
                if unsafe { *buffer.get_unchecked(i) } == b' ' {
                    i += 1;

                    // Parse y coordinate
                    let digits =
                        unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() } as usize;

                    digit = digits & 0xff;
                    if digit >= b'0' as usize && digit <= b'9' as usize {
                        y = digit - b'0' as usize;
                        i += 1;
                        digit = (digits >> 8) & 0xff;
                        if digit >= b'0' as usize && digit <= b'9' as usize {
                            y = 10 * y + digit - b'0' as usize;
                            i += 1;
                            digit = (digits >> 16) & 0xff;
                            if digit >= b'0' as usize && digit <= b'9' as usize {
                                y = 10 * y + digit - b'0' as usize;
                                i += 1;
                                digit = (digits >> 24) & 0xff;
                                if digit >= b'0' as usize && digit <= b'9' as usize {
                                    y = 10 * y + digit - b'0' as usize;
                                    i += 1;
                                }
                            }
                        }

                        x += connection_x_offset;
                        y += connection_y_offset;

                        // Separator between coordinates and color
                        if unsafe { *buffer.get_unchecked(i) } == b' ' {
                            i += 1;

                            // TODO: Determine what clients use more: RGB, RGBA or gg variant.
                            // If RGBA is used more often move the RGB code below the RGBA code

                            // Must be followed by 6 bytes RGB and newline or ...
                            if unsafe { *buffer.get_unchecked(i + 6) } == b'\n' {
                                last_byte_parsed = i + 6;
                                i += 7; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                let rgba: u32 = simd_unhex(&buffer[i - 7..i + 1]);

                                fb.set(x, y, rgba & 0x00ff_ffff);
                                continue;
                            }

                            // ... or must be followed by 8 bytes RGBA and newline
                            #[cfg(not(feature = "alpha"))]
                            if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                                last_byte_parsed = i + 8;
                                i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                let rgba: u32 = simd_unhex(&buffer[i - 9..i - 1]);

                                fb.set(x, y, rgba & 0x00ff_ffff);
                                continue;
                            }
                            #[cfg(feature = "alpha")]
                            if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                                last_byte_parsed = i + 8;
                                i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                let rgba = simd_unhex(&buffer[i - 9..i - 1]);

                                let alpha = (rgba >> 24) & 0xff;

                                if alpha == 0 || x >= fb.get_width() || y >= fb.get_height() {
                                    continue;
                                }

                                let alpha_comp = 0xff - alpha;
                                let current = fb.get_unchecked(x, y);
                                let r = (rgba >> 16) & 0xff;
                                let g = (rgba >> 8) & 0xff;
                                let b = rgba & 0xff;

                                let r: u32 =
                                    (((current >> 24) & 0xff) * alpha_comp + r * alpha) / 0xff;
                                let g: u32 =
                                    (((current >> 16) & 0xff) * alpha_comp + g * alpha) / 0xff;
                                let b: u32 =
                                    (((current >> 8) & 0xff) * alpha_comp + b * alpha) / 0xff;

                                fb.set(x, y, r << 16 | g << 8 | b);
                                continue;
                            }

                            // ... for the efficient/lazy clients
                            if unsafe { *buffer.get_unchecked(i + 2) } == b'\n' {
                                last_byte_parsed = i + 2;
                                i += 3; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                                let base = simd_unhex(&buffer[i - 3..i + 5]) & 0xff;

                                let rgba: u32 = base << 16 | base << 8 | base;

                                fb.set(x, y, rgba);

                                continue;
                            }
                        }

                        // End of command to read Pixel value
                        if unsafe { *buffer.get_unchecked(i) } == b'\n' {
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
        } else if current_command & 0x0000_ffff_ffff_ffff == string_to_number(b"OFFSET \0\0") {
            i += 7;
            // Parse x coordinate
            let digits =
                unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() } as usize;

            let mut digit = digits & 0xff;
            if digit >= b'0' as usize && digit <= b'9' as usize {
                x = digit - b'0' as usize;
                i += 1;
                digit = (digits >> 8) & 0xff;
                if digit >= b'0' as usize && digit <= b'9' as usize {
                    x = 10 * x + digit - b'0' as usize;
                    i += 1;
                    digit = (digits >> 16) & 0xff;
                    if digit >= b'0' as usize && digit <= b'9' as usize {
                        x = 10 * x + digit - b'0' as usize;
                        i += 1;
                        digit = (digits >> 24) & 0xff;
                        if digit >= b'0' as usize && digit <= b'9' as usize {
                            x = 10 * x + digit - b'0' as usize;
                            i += 1;
                        }
                    }
                }

                // Separator between x and y
                if unsafe { *buffer.get_unchecked(i) } == b' ' {
                    i += 1;

                    // Parse y coordinate
                    let digits =
                        unsafe { (buffer.as_ptr().add(i) as *const u32).read_unaligned() } as usize;

                    digit = digits & 0xff;
                    if digit >= b'0' as usize && digit <= b'9' as usize {
                        y = digit - b'0' as usize;
                        i += 1;
                        digit = (digits >> 8) & 0xff;
                        if digit >= b'0' as usize && digit <= b'9' as usize {
                            y = 10 * y + digit - b'0' as usize;
                            i += 1;
                            digit = (digits >> 16) & 0xff;
                            if digit >= b'0' as usize && digit <= b'9' as usize {
                                y = 10 * y + digit - b'0' as usize;
                                i += 1;
                                digit = (digits >> 24) & 0xff;
                                if digit >= b'0' as usize && digit <= b'9' as usize {
                                    y = 10 * y + digit - b'0' as usize;
                                    i += 1;
                                }
                            }
                        }

                        // End of command to set offset
                        if unsafe { *buffer.get_unchecked(i) } == b'\n' {
                            last_byte_parsed = i;
                            connection_x_offset = x;
                            connection_y_offset = y;
                            continue;
                        }
                    }
                }
            }
        } else if current_command & 0xffff_ffff == string_to_number(b"SIZE\0\0\0\0") {
            i += 4;
            last_byte_parsed = i - 1;

            stream
                .write_all(format!("SIZE {} {}\n", fb.get_width(), fb.get_height()).as_bytes())
                .await
                .expect("Failed to write bytes to tcp socket");
            continue;
        } else if current_command & 0xffff_ffff == string_to_number(b"HELP\0\0\0\0") {
            i += 4;
            last_byte_parsed = i - 1;

            stream
                .write_all(HELP_TEXT)
                .await
                .expect("Failed to write bytes to tcp socket");
            continue;
        }

        i += 1;
    }

    ParserState {
        connection_x_offset,
        connection_y_offset,
        last_byte_parsed,
    }
}

const SHIFT_PATTERN: Simd<u32, 8> = u32x8::from_array([4, 0, 12, 8, 20, 16, 28, 24]);
const SIMD_6: Simd<u32, 8> = u32x8::from_array([6; 8]);
const SIMD_F: Simd<u32, 8> = u32x8::from_array([0xf; 8]);
const SIMD_9: Simd<u32, 8> = u32x8::from_array([9; 8]);

/// Parse a slice of 8 characters into a single u32 number
/// is undefined behavior for invalid characters
#[inline(always)]
fn simd_unhex(value: &[u8]) -> u32 {
    #[cfg(debug_assertions)]
    assert_eq!(value.len(), 8);
    // Feel free to find a better, but fast, way, to cast all integers as u32
    let input = u32x8::from_array([
        value[0] as u32,
        value[1] as u32,
        value[2] as u32,
        value[3] as u32,
        value[4] as u32,
        value[5] as u32,
        value[6] as u32,
        value[7] as u32,
    ]);
    // Heavily inspired by https://github.com/nervosnetwork/faster-hex/blob/a4c06b387ddeeea311c9e84a3adcaf01015cf40e/src/decode.rs#L80
    let sr6 = input >> SIMD_6;
    let and15 = input & SIMD_F;
    let mul = sr6 * SIMD_9;
    let hexed = and15 + mul;
    let shifted = hexed << SHIFT_PATTERN;
    shifted.reduce_or()
}

pub fn check_cpu_support() {
    #[cfg(target_arch = "x86_64")]
    {
        if !is_x86_feature_detected!("avx2") {
            warn!("Your CPU does not support AVX2. Consider using a CPU supporting AVX2 for best performance");
        } else if !is_x86_feature_detected!("avx") {
            warn!("Your CPU does not support AVX. Consider using a CPU supporting AVX2 (or at least AVX) for best performance");
        } else {
            // At this point the CPU should support AVX und AVX2
            // Warn the user when he has compiled breakwater without the needed target features
            if cfg!(all(target_feature = "avx", target_feature = "avx2")) {
                info!("Using AVX and AVX2 support");
            } else {
                warn!("Your CPU does support AVX and AVX2, but you have not enabled avx and avx2 support. Please re-compile using RUSTFLAGS='-C target-cpu=native' cargo build --release`");
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_from_hex_char() {
        assert_eq!(simd_unhex(b"01234567"), 0x67452301);
        assert_eq!(simd_unhex(b"fedcba98"), 0x98badcfe);
    }
}
