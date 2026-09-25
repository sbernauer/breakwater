use std::{
    simd::{ToBytes, num::SimdUint, u8x8, u16x4},
    sync::Arc,
};

use crate::{
    ALT_HELP_TEXT, HELP_TEXT, MAX_HELP_CALLS_PER_CONNECTION, Parser, framebuffer::FrameBuffer,
};

/// The framebuffer capabilities [`OriginalParser`] requires.
///
/// With `binary-sync-pixels` the parser memcpys whole pixel runs into the framebuffer via
/// [`MultiPixelSet`], so the framebuffer must expose that. Otherwise plain [`FrameBuffer`] access
/// is enough.
#[cfg(feature = "binary-sync-pixels")]
pub trait OriginalParserFrameBuffer = FrameBuffer + crate::framebuffer::MultiPixelSet;
#[cfg(not(feature = "binary-sync-pixels"))]
pub trait OriginalParserFrameBuffer = FrameBuffer;

pub const PARSER_LOOKAHEAD: usize = "PX 1234 1234 rrggbbaa\n".len(); // Longest possible command

pub(crate) const PX_PATTERN: u64 = string_to_number(b"PX \0\0\0\0\0");
pub(crate) const PB_PATTERN: u64 = string_to_number(b"PB\0\0\0\0\0\0");
pub(crate) const OFFSET_PATTERN: u64 = string_to_number(b"OFFSET \0\0");
pub(crate) const SIZE_PATTERN: u64 = string_to_number(b"SIZE\0\0\0\0");
pub(crate) const HELP_PATTERN: u64 = string_to_number(b"HELP\0\0\0\0");
#[cfg(feature = "binary-sync-pixels")]
pub(crate) const PXMULTI_PATTERN: u64 = string_to_number(b"PXMULTI\0");

pub struct OriginalParser<FB: FrameBuffer> {
    connection_x_offset: usize,
    connection_y_offset: usize,
    /// How often the client requested the help on this connection. It is tracked per connection.
    help_count: u8,
    fb: Arc<FB>,
    #[cfg(feature = "binary-sync-pixels")]
    remaining_pixel_sync: Option<RemainingPixelSync>,
}

#[cfg(feature = "binary-sync-pixels")]
#[derive(Debug)]
pub struct RemainingPixelSync {
    current_index: usize,
    bytes_remaining: usize,
}

impl<FB: FrameBuffer> OriginalParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self {
            connection_x_offset: 0,
            connection_y_offset: 0,
            help_count: 0,
            fb,
            #[cfg(feature = "binary-sync-pixels")]
            remaining_pixel_sync: None,
        }
    }
}

impl<FB: OriginalParserFrameBuffer> Parser for OriginalParser<FB> {
    #[allow(clippy::too_many_lines)]
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize {
        // As this is a potentially(?) expensive operation we only call it one in this parsing loop
        // All the pixels likely where in the same TCP packets (+- 1/2 or so) it doesn't matter after all
        // Encode the timestamp exactly once here, not per pixel: it's constant for the whole parse
        // call, so computing it per write would just waste throughput on the hot path.
        let current_ts = self.fb.current_ts();

        let mut last_byte_parsed = 0;

        let mut i = 0; // We can't use a for loop here because Rust don't lets use skip characters by incrementing i
        let loop_end = buffer.len().saturating_sub(PARSER_LOOKAHEAD); // Let's extract the .len() call and the subtraction into it's own variable so we only compute it once

        #[cfg(feature = "binary-sync-pixels")]
        if let Some(remaining) = &self.remaining_pixel_sync {
            let buffer = &buffer[0..loop_end];

            if remaining.bytes_remaining <= buffer.len() {
                // Easy going here
                self.fb
                    .set_multi_from_start_index(remaining.current_index, unsafe {
                        core::slice::from_raw_parts(buffer.as_ptr(), remaining.bytes_remaining)
                    });
                i += remaining.bytes_remaining;
                last_byte_parsed = i;
                self.remaining_pixel_sync = None;
            } else {
                // The client requested to write more bytes that are currently in the buffer, we need to remember
                // what the client is doing.

                // We need to round down to the 4 bytes of a pixel alignment
                let pixel_bytes = buffer.len() / 4 * 4;

                let mut index = remaining.current_index;
                index += self
                    .fb
                    .set_multi_from_start_index(remaining.current_index, unsafe {
                        core::slice::from_raw_parts(buffer.as_ptr(), pixel_bytes)
                    });

                self.remaining_pixel_sync = Some(RemainingPixelSync {
                    current_index: index,
                    bytes_remaining: remaining.bytes_remaining.saturating_sub(pixel_bytes),
                });

                // Nothing to do left, we can early return
                // I have absolutely no idea why we need to subtract 1 here, but it is what it is. At least we have
                // tests for this madness :)
                return i + pixel_bytes.saturating_sub(1);
            }
        }

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

                            self.fb.set(x, y, rgba | 0xff00_0000, current_ts);
                            continue;
                        }

                        // ... or must be followed by 8 bytes RGBA and newline
                        #[cfg(not(feature = "alpha"))]
                        if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                            last_byte_parsed = i + 8;
                            i += 9; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(i - 9) });

                            self.fb.set(x, y, rgba | 0xff00_0000, current_ts);
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
                            let current = unsafe { self.fb.get_unchecked(x, y) };
                            let red = (rgba >> 16) & 0xff;
                            let green = (rgba >> 8) & 0xff;
                            let blue = rgba & 0xff;

                            let red: u32 =
                                (((current >> 24) & 0xff) * alpha_comp + red * alpha) / 0xff;
                            let green: u32 =
                                (((current >> 16) & 0xff) * alpha_comp + green * alpha) / 0xff;
                            let blue: u32 =
                                (((current >> 8) & 0xff) * alpha_comp + blue * alpha) / 0xff;

                            self.fb.set(
                                x,
                                y,
                                (red << 16) | (green << 8) | blue | 0xff00_0000,
                                current_ts,
                            );
                            continue;
                        }

                        // ... for the efficient/lazy clients
                        if unsafe { *buffer.get_unchecked(i + 2) } == b'\n' {
                            last_byte_parsed = i + 2;
                            i += 3; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                            let base = simd_unhex(unsafe { buffer.as_ptr().add(i - 3) }) & 0xff;

                            let rgba: u32 = (base << 16) | (base << 8) | base;

                            self.fb.set(x, y, rgba | 0xff00_0000, current_ts);

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
            }
            #[cfg(feature = "binary-set-pixel")]
            if current_command & 0x0000_ffff == PB_PATTERN {
                let command_bytes =
                    unsafe { (buffer.as_ptr().add(i + 2) as *const u64).read_unaligned() };

                let x = u16::from_le((command_bytes) as u16);
                let y = u16::from_le((command_bytes >> 16) as u16);
                let rgba = u32::from_le((command_bytes >> 32) as u32);

                // TODO: Support alpha channel (behind alpha feature flag)
                self.fb
                    .set(x as usize, y as usize, rgba & 0x00ff_ffff, current_ts);
                //                 P   B   XX  YY  RGBA
                last_byte_parsed = i + 1 + 2 + 2 + 4;
                i += 10;
                continue;
            }
            #[cfg(feature = "binary-sync-pixels")]
            if current_command & 0x00ff_ffff_ffff_ffff == PXMULTI_PATTERN {
                i += "PXMULTI".len();
                let header = unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() };
                i += 8;

                let start_x = u16::from_le((header) as u16);
                let start_y = u16::from_le((header >> 16) as u16);
                let len = u32::from_le((header >> 32) as u32);
                let len_in_bytes = len as usize * 4;
                let bytes_left_in_buffer = loop_end.saturating_sub(i);

                if len_in_bytes <= bytes_left_in_buffer {
                    // Easy going here
                    self.fb
                        .set_multi(start_x as usize, start_y as usize, unsafe {
                            core::slice::from_raw_parts(buffer.as_ptr().add(i), len_in_bytes)
                        });

                    i += len_in_bytes;
                    last_byte_parsed = i;
                    continue;
                }

                // We need to round down to the 4 bytes of a pixel alignment
                let pixel_bytes: usize = bytes_left_in_buffer / 4 * 4;

                // The client requested to write more bytes that are currently in the buffer, we need to remember
                // what the client is doing.
                let mut current_index = start_x as usize + start_y as usize * self.fb.get_width();
                current_index += self.fb.set_multi_from_start_index(current_index, unsafe {
                    core::slice::from_raw_parts(buffer.as_ptr().add(i), pixel_bytes)
                });

                self.remaining_pixel_sync = Some(RemainingPixelSync {
                    current_index,
                    bytes_remaining: len_in_bytes - pixel_bytes,
                });

                // Nothing to do left, we can early return
                // I have absolutely no idea why we need to subtract 1 here, but it is what it is. At least we have
                // tests for this madness :)
                return i + pixel_bytes.saturating_sub(1);
            }
            if current_command & 0x00ff_ffff_ffff_ffff == OFFSET_PATTERN {
                i += 7;

                let (x, y, present) = parse_pixel_coordinates(buffer.as_ptr(), &mut i);

                // End of command to set offset
                if present && unsafe { *buffer.get_unchecked(i) } == b'\n' {
                    last_byte_parsed = i;
                    self.connection_x_offset = x;
                    self.connection_y_offset = y;
                    continue;
                }
            }
            if current_command & 0xffff_ffff == SIZE_PATTERN {
                i += 4;
                last_byte_parsed = i + 1;

                response.extend_from_slice(
                    format!("SIZE {} {}\n", self.fb.get_width(), self.fb.get_height()).as_bytes(),
                );
                continue;
            }
            if current_command & 0xffff_ffff == HELP_PATTERN {
                i += 4;
                last_byte_parsed = i + 1;

                match self.help_count {
                    0..MAX_HELP_CALLS_PER_CONNECTION => {
                        response.extend_from_slice(HELP_TEXT);
                        self.help_count += 1;
                    }
                    MAX_HELP_CALLS_PER_CONNECTION => {
                        response.extend_from_slice(ALT_HELP_TEXT);
                        self.help_count += 1;
                    }
                    _ => {
                        // The client has requested the help to often, let's just ignore it
                    }
                }
                continue;
            }

            i += 1;
        }

        last_byte_parsed
        // last_byte_parsed.saturating_sub(1)
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
    }
}

const fn string_to_number(input: &[u8]) -> u64 {
    ((input[7] as u64) << 56)
        | ((input[6] as u64) << 48)
        | ((input[5] as u64) << 40)
        | ((input[4] as u64) << 32)
        | ((input[3] as u64) << 24)
        | ((input[2] as u64) << 16)
        | ((input[1] as u64) << 8)
        | (input[0] as u64)
}

/// Parses 8 hex characters into a u32, the first two characters end up in the least significant
/// byte. Invalid characters result in some garbage color, but never in undefined behavior.
///
/// All characters are processed at once in 8 u8 lanes, which only needs SSE2 on x86. This is a
/// lot faster than working on 8 u32 lanes (which needs variable shifts and a horizontal reduction)
/// and also faster than doing the same in a general purpose register, as the parsing loop already
/// keeps the integer ALUs busy.
#[inline(always)]
pub(crate) fn simd_unhex(value: *const u8) -> u32 {
    let chars = u8x8::from_array(unsafe { (value as *const [u8; 8]).read_unaligned() });

    // Per character `(char & 0xf) + (char >> 6) * 9`, which is 0-15 for `0-9`, `a-f` and `A-F`.
    // Inspired by https://github.com/nervosnetwork/faster-hex/blob/a4c06b387ddeeea311c9e84a3adcaf01015cf40e/src/decode.rs#L80
    let nibbles = (chars & u8x8::splat(0xf)) + (chars >> u8x8::splat(6)) * u8x8::splat(9);

    // Every u16 lane holds the two characters forming one byte of the result, the first one being
    // the high nibble. The truncating cast drops everything that got shifted beyond that byte.
    let pairs = u16x4::from_le_bytes(nibbles);
    let bytes = ((pairs << u16x4::splat(4)) | (pairs >> u16x4::splat(8))).cast::<u8>();
    u32::from_le_bytes(bytes.to_array())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SimpleFrameBuffer;

    #[test]
    fn help_is_rate_limited_across_parse_calls() {
        let mut parser = OriginalParser::new(Arc::new(SimpleFrameBuffer::new(640, 480)));

        // Every parse call simulates a separate read from the socket, which (as the server does
        // it) is followed by PARSER_LOOKAHEAD zeroed bytes
        let mut buffer = b"HELP\n".to_vec();
        buffer.resize(buffer.len() + PARSER_LOOKAHEAD, 0);

        let expected: [&[u8]; 6] = [HELP_TEXT, HELP_TEXT, HELP_TEXT, ALT_HELP_TEXT, b"", b""];
        for (call, expected) in expected.into_iter().enumerate() {
            let mut response = Vec::new();
            parser.parse(&buffer, &mut response);
            assert_eq!(
                response, expected,
                "Unexpected response to HELP on parse call {call}"
            );
        }
    }

    /// Parses every pair of hex digits on its own, the first pair ends up in the least significant
    /// byte
    fn unhex_reference(chars: [u8; 8]) -> u32 {
        let bytes: [u8; 4] = std::array::from_fn(|pair| {
            let pair = std::str::from_utf8(&chars[pair * 2..pair * 2 + 2]).expect("Not utf-8");
            u8::from_str_radix(pair, 16).expect("Not a hex number")
        });
        u32::from_le_bytes(bytes)
    }

    /// Only valid hex digits need to be parsed correctly, invalid ones can result in any color
    #[test]
    fn simd_unhex_matches_reference() {
        const HEX_DIGITS: &[u8] = b"0123456789abcdefABCDEF";

        // xorshift64, we don't want a dependency just to get some random bytes
        let mut state = 0x1234_5678_9abc_def0_u64;
        for _ in 0..1_000_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;

            let chars = state
                .to_le_bytes()
                .map(|byte| HEX_DIGITS[byte as usize % HEX_DIGITS.len()]);

            assert_eq!(
                simd_unhex(chars.as_ptr()),
                unhex_reference(chars),
                "Different result for {chars:?}"
            );
        }
    }

    #[test]
    fn simd_unhex_parses_hex() {
        assert_eq!(simd_unhex(b"12345678".as_ptr()), 0x7856_3412);
        assert_eq!(simd_unhex(b"abcdefAB".as_ptr()), 0xabef_cdab);
        assert_eq!(simd_unhex(b"ABCDEFab".as_ptr()), 0xabef_cdab);
        assert_eq!(simd_unhex(b"00ff00ff".as_ptr()), 0xff00_ff00);
        // RGB followed by the newline and the next command
        assert_eq!(simd_unhex(b"c0ffee\nP".as_ptr()) & 0x00ff_ffff, 0x00ee_ffc0);
    }
}
