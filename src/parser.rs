use crate::framebuffer::FrameBuffer;
use const_format::formatcp;
use log::{info, warn};
use std::simd::{
    u16x16, u16x32, u16x4, u16x8, u32x8, u8x32, u8x8, Simd, SimdPartialOrd, SimdUint, ToBitMask,
};
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

            // TODO: Use variant that does not check bounds to get &buffer[i..i + 19]
            let ParseResult {
                bytes_parsed,
                x,
                y,
                rgba,
                has_alpha,
                read_request,
            } = parse_coords_and_rgba(&buffer[i..i + 19]);
            // let x = 0;
            // let y = 0;
            // let rgba = 0;
            // let read_request = false;
            // let bytes_parsed = 17;
            i += bytes_parsed;
            last_byte_parsed = i - 1;

            let x = x as usize + connection_x_offset;
            let y = y as usize + connection_y_offset;

            if !read_request {
                fb.set(x, y, rgba & 0x00ff_ffff);
                continue;
            } else if let Some(rgb) = fb.get(x, y) {
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
                continue;
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

// Longest possible space bitmask = "1234 1234 rrggbbaa\n" => 19 chars
const SPACES_BITMASK_RELEVANT_BITS: u8 = 19;
const SPACES_BITMASK_RELEVANT_BITS_MASK: u32 = 0b0000_0000_0111_1111_1111_1111_1111;
const FACTORS_FOR_BITMASK: [(u16x32, u16x32, u16x32, u16x32);
    (1 << SPACES_BITMASK_RELEVANT_BITS) + 1] = calculate_factors_for_bitmask();
const SIMD_u16x16_9: u16x16 = u16x16::from_array([9; 16]);
const SIMD_u16x32_9: u16x32 = u16x32::from_array([9; 32]);
const SIMD_u16x16_0_CHAR: u16x16 = u16x16::from_array([b'0' as u16; 16]);
const SIMD_u16x32_0_CHAR: u16x32 = u16x32::from_array([b'0' as u16; 32]);
const SIMD_u16x8_0_CHAR: u16x8 = u16x8::from_array([b'0' as u16; 8]);

struct ParseResult {
    bytes_parsed: usize,
    x: u16,
    y: u16,
    // aabbggrr
    rgba: u32,
    read_request: bool,
    has_alpha: bool,
}

// Input: 19 characters starting where the x coordinate starts, eg. "1234 4321 <rr|rrggbb|rrggbbaa>\n<random chars follow>"
// Returns: (x, y, total length of text containing "<x> <y>")
//
// Inspect assembler code using
// RUSTFLAGS="-C target-cpu=native" CARGO_INCREMENTAL=0 cargo -Z build-std asm --build-type release --rust breakwater::parser::parse_coords_and_rgba
// Don't forget to #[inline(never)]!
#[inline(never)]
fn parse_coords_and_rgba(bytes: &[u8]) -> ParseResult {
    // #[cfg(debug_assertions)]
    assert!(bytes.len() >= 19);
    let chars = u16x32::from_array([
        bytes[0] as u16,
        bytes[1] as u16,
        bytes[2] as u16,
        bytes[3] as u16,
        bytes[4] as u16,
        bytes[5] as u16,
        bytes[6] as u16,
        bytes[7] as u16,
        bytes[8] as u16,
        bytes[9] as u16,
        bytes[10] as u16,
        bytes[11] as u16,
        bytes[12] as u16,
        bytes[13] as u16,
        bytes[14] as u16,
        bytes[15] as u16,
        bytes[16] as u16,
        bytes[17] as u16,
        bytes[18] as u16,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ]);
    let digits = chars - SIMD_u16x32_0_CHAR;
    let space_bitmask = digits.simd_gt(SIMD_u16x32_9).to_bitmask();
    // SAFETY: As only take the last {SPACES_BITMASK_RELEVANT_BITS} bits, this number will alway be a valid index
    let (x_factors, y_factors, rg_factors, ba_factors) =
        unsafe { FACTORS_FOR_BITMASK.get_unchecked(space_bitmask as usize) };
    // let (x_factors, y_factors, rg_factors, ba_factors) =
    //     FACTORS_FOR_BITMASK[space_bitmask as usize];
    let x = (digits * x_factors).reduce_sum();
    let y = (digits * y_factors).reduce_sum();
    let rg = (digits * rg_factors).reduce_sum();
    let ba = (digits * ba_factors).reduce_sum();
    // let x = 0;
    // let y = 0;
    // let rg = 0;
    // let ba = 0;

    ParseResult {
        bytes_parsed: 17,
        x,
        y,
        rgba: (rg as u32) << 16 | ba as u32,
        has_alpha: false,
        read_request: false,
    }
}

// PX 13 123 rrggbbaa

// const TEST: u16x32 = u16x32::from_array([
//     8, 0, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
//     16, 16, 16, 16, 16, 16, 16, 16,
// ]);
// const TEST2: u16x32 = u16x32::from_array([
//     2, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16,
//     16, 16, 16, 16, 16, 16, 16, 16,
// ]);
// #[inline(never)]
// fn parse_coords_and_rgba(bytes: &[u8]) -> ParseResult {
//     #[cfg(debug_assertions)]
//     assert!(bytes.len() >= 19);
//     let chars = u16x32::from_array([
//         bytes[0] as u16,
//         bytes[1] as u16,
//         bytes[2] as u16,
//         bytes[3] as u16,
//         bytes[4] as u16,
//         bytes[5] as u16,
//         bytes[6] as u16,
//         bytes[7] as u16,
//         bytes[8] as u16,
//         bytes[9] as u16,
//         bytes[10] as u16,
//         bytes[11] as u16,
//         bytes[12] as u16,
//         bytes[13] as u16,
//         bytes[14] as u16,
//         bytes[15] as u16,
//         bytes[16] as u16,
//         bytes[17] as u16,
//         bytes[18] as u16,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//     ]);
//     let digits = chars - SIMD_u16x32_0_CHAR;
//     // let space_bitmask = digits.simd_gt(SIMD_u16x16_9).to_bitmask();

//     // let num = unsafe {
//     //     std::mem::transmute::<[u8; 4], u32>(raw_bytes)
//     // };
//     // let hack: u16x8 = unsafe { std::mem::transmute::<u16x32, u16x8>(digits) };

//     let x_1 = digits << TEST;
//     let x_2 = digits << TEST2;

//     let x = (x_1 + x_2).reduce_or();
//     let y = (y_1 + y_2).reduce_or();

//     // SAFETY: As only take the last {SPACES_BITMASK_RELEVANT_BITS} bits, this number will alway be a valid index
//     // let (x_factors, y_factors, rg_factors, ba_factors) =
//     //     unsafe { FACTORS_FOR_BITMASK.get_unchecked(space_bitmask as usize) };
//     // let (x_factors, y_factors, rg_factors, ba_factors) =
//     //     FACTORS_FOR_BITMASK[space_bitmask as usize];
//     // let x = (digits * x_factors).reduce_sum();

//     // let y = (digits * y_factors).reduce_sum();
//     // let rg = (digits * rg_factors).reduce_sum();
//     // let ba = (digits * ba_factors).reduce_sum();
//     // let x = space_bitmask;
//     let rg = 0;
//     let ba = 0;

//     ParseResult {
//         bytes_parsed: 17,
//         x,
//         y,
//         rgba: (rg as u32) << 16 | ba as u32,
//         has_alpha: false,
//         read_request: false,
//     }
// }

const fn calculate_factors_for_bitmask(
) -> [(u16x32, u16x32, u16x32, u16x32); (1 << SPACES_BITMASK_RELEVANT_BITS) + 1] {
    let mut result = [(
        u16x32::from_array([0; 32]),
        u16x32::from_array([0; 32]),
        u16x32::from_array([0; 32]),
        u16x32::from_array([0; 32]),
    ); (1 << SPACES_BITMASK_RELEVANT_BITS) + 1];

    result
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
