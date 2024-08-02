use std::{
    simd::{num::SimdUint, u32x8, Simd},
    sync::Arc,
};

use crate::{FrameBuffer, Parser};

pub const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command

pub(crate) const PX_PATTERN: u64 = string_to_number(b"PX \0\0\0\0\0");
pub(crate) const PB_PATTERN: u64 = string_to_number(b"PB\0\0\0\0\0\0");
pub(crate) const OFFSET_PATTERN: u64 = string_to_number(b"OFFSET \0\0");
pub(crate) const SIZE_PATTERN: u64 = string_to_number(b"SIZE\0\0\0\0");
pub(crate) const HELP_PATTERN: u64 = string_to_number(b"HELP\0\0\0\0");

pub struct OriginalParser<FB: FrameBuffer> {
    connection_x_offset: usize,
    connection_y_offset: usize,
    help_text: &'static [u8],
    alt_help_text: &'static [u8],
    fb: Arc<FB>,
}

impl<FB: FrameBuffer> OriginalParser<FB> {
    pub fn new(fb: Arc<FB>, help_text: &'static [u8], alt_help_text: &'static [u8]) -> Self {
        Self {
            connection_x_offset: 0,
            connection_y_offset: 0,
            fb,
            help_text,
            alt_help_text,
        }
    }
}

impl<FB: FrameBuffer> Parser for OriginalParser<FB> {
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize {
        let mut last_byte_parsed = 0;
        let mut help_count = 0;

        let mut i = 0; // We can't use a for loop here because Rust don't lets use skip characters by incrementing i
        let loop_end = buffer.len().saturating_sub(PARSER_LOOKAHEAD); // Let's extract the .len() call and the subtraction into it's own variable so we only compute it once

        while i < loop_end {
            let current_command =
                unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() };
            if current_command & 0x00ff_ffff == PX_PATTERN {
                i += 3;

                let (mut x, mut y, present) = parse_pixel_coordinates(buffer.as_ptr(), &mut i);

                if present {
                    x += self.connection_x_offset;
                    y += self.connection_y_offset;

                    // Separator between coordinates and color
                    if unsafe { *buffer.get_unchecked(i) } == b' ' {
                        i += 1;

                        // TODO: Determine what clients use more: RGB, RGBA or gg variant.
                        // If RGBA is used more often move the RGB code below the RGBA code

                        // Must be followed by 6 bytes RGB and newline or ...
                        if unsafe { *buffer.get_unchecked(i + 6) } == b'\n' {
                            last_byte_parsed = i + 6;
                            i += 7; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(i - 7) });

                            self.fb.set(x, y, rgba & 0x00ff_ffff);
                            continue;
                        }

                        // ... or must be followed by 8 bytes RGBA and newline
                        #[cfg(not(feature = "alpha"))]
                        if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                            last_byte_parsed = i + 8;
                            i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(i - 9) });

                            self.fb.set(x, y, rgba & 0x00ff_ffff);
                            continue;
                        }
                        #[cfg(feature = "alpha")]
                        if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                            last_byte_parsed = i + 8;
                            i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let rgba = simd_unhex(unsafe { buffer.as_ptr().add(i - 9) });

                            let alpha = (rgba >> 24) & 0xff;

                            if alpha == 0 || x >= self.fb.get_width() || y >= self.fb.get_height() {
                                continue;
                            }

                            let alpha_comp = 0xff - alpha;
                            let current = self.fb.get_unchecked(x, y);
                            let r = (rgba >> 16) & 0xff;
                            let g = (rgba >> 8) & 0xff;
                            let b = rgba & 0xff;

                            let r: u32 = (((current >> 24) & 0xff) * alpha_comp + r * alpha) / 0xff;
                            let g: u32 = (((current >> 16) & 0xff) * alpha_comp + g * alpha) / 0xff;
                            let b: u32 = (((current >> 8) & 0xff) * alpha_comp + b * alpha) / 0xff;

                            self.fb.set(x, y, r << 16 | g << 8 | b);
                            continue;
                        }

                        // ... for the efficient/lazy clients
                        if unsafe { *buffer.get_unchecked(i + 2) } == b'\n' {
                            last_byte_parsed = i + 2;
                            i += 3; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let base = simd_unhex(unsafe { buffer.as_ptr().add(i - 3) }) & 0xff;

                            let rgba: u32 = base << 16 | base << 8 | base;

                            self.fb.set(x, y, rgba);

                            continue;
                        }
                    }

                    // End of command to read Pixel value
                    if unsafe { *buffer.get_unchecked(i) } == b'\n' {
                        last_byte_parsed = i;
                        i += 1;
                        if let Some(rgb) = self.fb.get(x, y) {
                            response.extend_from_slice(
                                format!(
                                    "PX {} {} {:06x}\n",
                                    // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                                    x - self.connection_x_offset,
                                    y - self.connection_y_offset,
                                    rgb.to_be() >> 8
                                )
                                .as_bytes(),
                            );
                        }
                        continue;
                    }
                }
            // In case the feature is disabled this if should be optimized away, as "cfg!" should be a constant expression.
            } else if cfg!(feature = "binary-commands")
                && current_command & 0x0000_ffff == PB_PATTERN
            {
                let command_bytes =
                    unsafe { (buffer.as_ptr().add(i + 2) as *const u64).read_unaligned() };

                let x = u16::from_le((command_bytes) as u16);
                let y = u16::from_le((command_bytes >> 16) as u16);
                let rgba = u32::from_le((command_bytes >> 32) as u32);

                // TODO: Support alpha channel (behind alpha feature flag)
                self.fb.set(x as usize, y as usize, rgba & 0x00ff_ffff);

                i += 10;
                continue;
            } else if current_command & 0x00ff_ffff_ffff_ffff == OFFSET_PATTERN {
                i += 7;

                let (x, y, present) = parse_pixel_coordinates(buffer.as_ptr(), &mut i);

                // End of command to set offset
                if present && unsafe { *buffer.get_unchecked(i) } == b'\n' {
                    last_byte_parsed = i;
                    self.connection_x_offset = x;
                    self.connection_y_offset = y;
                    continue;
                }
            } else if current_command & 0xffff_ffff == SIZE_PATTERN {
                i += 4;
                last_byte_parsed = i - 1;

                response.extend_from_slice(
                    format!("SIZE {} {}\n", self.fb.get_width(), self.fb.get_height()).as_bytes(),
                );
                continue;
            } else if current_command & 0xffff_ffff == HELP_PATTERN {
                i += 4;
                last_byte_parsed = i - 1;

                match help_count {
                    0..=2 => {
                        response.extend_from_slice(self.help_text);
                        help_count += 1;
                    }
                    3 => {
                        response.extend_from_slice(self.alt_help_text);
                        help_count += 1;
                    }
                    _ => {
                        // The client has requested the help to often, let's just ignore it
                    }
                }
                continue;
            }

            i += 1;
        }

        last_byte_parsed.wrapping_sub(1)
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
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

const SHIFT_PATTERN: Simd<u32, 8> = u32x8::from_array([4, 0, 12, 8, 20, 16, 28, 24]);
const SIMD_6: Simd<u32, 8> = u32x8::from_array([6; 8]);
const SIMD_F: Simd<u32, 8> = u32x8::from_array([0xf; 8]);
const SIMD_9: Simd<u32, 8> = u32x8::from_array([9; 8]);

/// Parse a slice of 8 characters into a single u32 number
/// is undefined behavior for invalid characters
#[inline(always)]
pub(crate) fn simd_unhex(value: *const u8) -> u32 {
    // Feel free to find a better, but fast, way, to cast all integers as u32
    let input = unsafe {
        u32x8::from_array([
            *value as u32,
            *value.add(1) as u32,
            *value.add(2) as u32,
            *value.add(3) as u32,
            *value.add(4) as u32,
            *value.add(5) as u32,
            *value.add(6) as u32,
            *value.add(7) as u32,
        ])
    };
    // Heavily inspired by https://github.com/nervosnetwork/faster-hex/blob/a4c06b387ddeeea311c9e84a3adcaf01015cf40e/src/decode.rs#L80
    let sr6 = input >> SIMD_6;
    let and15 = input & SIMD_F;
    let mul = sr6 * SIMD_9;
    let hexed = and15 + mul;
    let shifted = hexed << SHIFT_PATTERN;
    shifted.reduce_or()
}

#[inline(always)]
fn parse_coordinate(buffer: *const u8, current_index: &mut usize) -> (usize, bool) {
    let digits = unsafe { (buffer.add(*current_index) as *const usize).read_unaligned() };

    let mut result = 0;
    let mut visited = false;
    // The compiler will unroll this loop, but this way, it is more maintainable
    for pos in 0..4 {
        let digit = (digits >> (pos * 8)) & 0xff;
        if digit >= b'0' as usize && digit <= b'9' as usize {
            result = 10 * result + digit - b'0' as usize;
            *current_index += 1;
            visited = true;
        } else {
            break;
        }
    }

    (result, visited)
}

#[inline(always)]
pub(crate) fn parse_pixel_coordinates(
    buffer: *const u8,
    current_index: &mut usize,
) -> (usize, usize, bool) {
    let (x, x_visited) = parse_coordinate(buffer, current_index);
    *current_index += 1;
    let (y, y_visited) = parse_coordinate(buffer, current_index);
    (x, y, x_visited && y_visited)
}
