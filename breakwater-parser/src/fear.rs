use std::ops::Sub;
use std::sync::Arc;

use fearless_simd::{Bytes, Level, Simd, SimdFrom, SimdMask, dispatch};
use fearless_simd::{SimdBase, u8x32, u16x16};
use fearless_simd_macros::simd;

use crate::original::{HELP_PATTERN, PX_PATTERN};
use crate::{ALT_HELP_TEXT, FrameBuffer, HELP_TEXT, MAX_HELP_CALLS_PER_CONNECTION, Parser};

/// We work on 32 byte vectors
pub const PARSER_LOOKAHEAD: usize = 32;

// Longest possible space bitmask = "1234 1234 " => 10 chars
const SPACES_BITMASK_MASK: u16 = 0b0000_0000_0000_0000_0011_1111_1111;

static SHUFFLE_PATTERNS: [(u8, [u8; 32]); u16::MAX as usize + 1] =
    manually_calculated_shuffle_patterns();

pub struct FearParser<FB: FrameBuffer> {
    /// How often the client requested the help on this connection. It is tracked per connection.
    help_count: u8,
    fb: Arc<FB>,
    simd_level: Level,
}

impl<FB: FrameBuffer> FearParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self {
            help_count: 0,
            fb,
            simd_level: Level::new(),
        }
    }
}

impl<FB: FrameBuffer> Parser for FearParser<FB> {
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

        while i < loop_end {
            // let next = &buffer[i..][..10];
            // dbg!(String::from_utf8_lossy(next));

            let current_command =
                unsafe { (buffer.as_ptr().add(i) as *const u64).read_unaligned() };
            if current_command & 0x00ff_ffff == PX_PATTERN {
                i += 3;

                let (x, y, rgb, bytes_parsed) = dispatch!(self.simd_level, simd => simd_parse(simd, unsafe { buffer.as_ptr().add(i)}));

                if bytes_parsed > 0 {
                    last_byte_parsed = i + bytes_parsed as usize;
                    i += bytes_parsed as usize + 1; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                    self.fb
                        .set(x as usize, y as usize, rgb & 0x00ff_ffff, current_ts);
                    continue;
                }

                // if present {
                //     // Separator between coordinates and color
                //     if unsafe { *buffer.get_unchecked(i) } == b' ' {
                //         i += 1;

                //         // TODO: Determine what clients use more: RGB, RGBA or gg variant.
                //         // If RGBA is used more often move the RGB code below the RGBA code

                //         // Must be followed by 6 bytes RGB and newline or ...
                //         if unsafe { *buffer.get_unchecked(i + 6) } == b'\n' {
                //             last_byte_parsed = i + 6;
                //             i += 7; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                //             let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(i - 7) });

                //             self.fb.set(x, y, rgba & 0x00ff_ffff, current_ts);
                //             continue;
                //         }

                //         // ... or must be followed by 8 bytes RGBA and newline
                //         #[cfg(not(feature = "alpha"))]
                //         if unsafe { *buffer.get_unchecked(i + 8) } == b'\n' {
                //             last_byte_parsed = i + 8;
                //             i += 9; // We can advance one byte more than normal - self.connection_y_offsetas we use continue and therefore not get incremented at the end of the loop

                //             let rgba: u32 = simd_unhex(unsafe { buffer.as_ptr().add(i - 9) });

                //             self.fb.set(x, y, rgba & 0x00ff_ffff, current_ts);
                //             continue;
                //         }
                //     }

                //     // End of command to read Pixel value
                //     if unsafe { *buffer.get_unchecked(i) } == b'\n' {
                //         last_byte_parsed = i;
                //         i += 1;
                //         if let Some(rgb) = self.fb.get(x, y) {
                //             response.extend_from_slice(
                //                 format!(
                //                     "PX {} {} {:06x}\n",
                //                     // We don't want to return the actual (absolute) coordinates, the client should also get the result offseted
                //                     x,
                //                     y,
                //                     rgb.to_be() >> 8
                //                 )
                //                 .as_bytes(),
                //             );
                //         }
                //         continue;
                //     }
                // }
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

#[simd]
fn simd_parse<S: Simd>(simd: S, buffer: *const u8) -> (u16, u16, u32, u8) {
    // Constants
    let simd_0_chars = u8x32::splat(simd, b'0');
    let simd_spaces = u8x32::splat(simd, b' ');
    let simd_newlines = u8x32::splat(simd, b'\n');
    let decimal_factors_x =
        u16x16::simd_from(simd, [1000, 100, 10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let decimal_factors_y =
        u16x16::simd_from(simd, [0, 0, 0, 0, 1000, 100, 10, 1, 0, 0, 0, 0, 0, 0, 0, 0]);

    // Actual code starts here
    let buffer_first_32 = unsafe { (buffer as *const [u8; 32]).read_unaligned() };

    let chars = u8x32::simd_from(simd, buffer_first_32);
    let digits = chars.sub(simd_0_chars);

    let spaces_bitmask = chars.simd_eq(simd_spaces).to_bitmask() as u16;
    let newlines_bitmask = chars.simd_eq(simd_newlines).to_bitmask() as u16;
    let spaces_bitmask = (spaces_bitmask | newlines_bitmask) & SPACES_BITMASK_MASK;

    // dbg!(format!("{spaces_bitmask:032b}"));

    // SAFETY: As SHUFFLE_PATTERNS has length `u16::MAX as usize + 1` and we use a us16 to index into it it will always succeed
    let (bytes_parsed, shuffle_pattern) =
        unsafe { *SHUFFLE_PATTERNS.get_unchecked(spaces_bitmask as usize) };
    let shuffle_pattern = u8x32::simd_from(simd, shuffle_pattern);

    // This swizzles the input digits (ASCII - b'0') so that x is at byte 0-3, y at byte 4-7 and
    // rgb at bytes 8-10.
    let digits = digits.swizzle_dyn_precise(shuffle_pattern);
    let digits = u16x16::from_bytes(digits);

    let x = (digits * decimal_factors_x).reduce_sum();
    let y = (digits * decimal_factors_y).reduce_sum();

    // After subtracting b'0': `0-9` -> 0x00-0x09, `A-F` -> 0x11-0x16, `a-f` -> 0x31-0x36.
    // Bit 4 is set exactly for letters, whose low nibble is 1-6, so per byte
    // `(d & 0xf) + ((d >> 4) & 1) * 9` yields 0-15. Every byte stays <= 24, so nothing carries into
    // the neighbouring byte and both characters of a u16 lane are handled at once. Decimal digits
    // and zeroed lanes are fixed points, so this doesn't affect x and y.
    let hex = (digits & 0x0f0f) + ((digits >> 4) & 0x0101) * 9;

    // Low byte of every lane becomes `(first << 4) | second`; the truncating casts below drop the rest.
    let rgb = (hex << 4) | (hex >> 8);
    let rgb = u32::from_le_bytes([rgb[10] as u8, rgb[9] as u8, rgb[8] as u8, 0]);

    (x, y, rgb, bytes_parsed)
}

// Let's add the stuff manually, we can always automate later
#[allow(clippy::too_many_lines)]
#[allow(clippy::large_stack_arrays)] // TODO: Think about this
const fn manually_calculated_shuffle_patterns() -> [(u8, [u8; 32]); u16::MAX as usize + 1] {
    let mut shuffle_patterns = [(0, [255; 32]); u16::MAX as usize + 1];

    // 9 9
    shuffle_patterns[0b0000_0000_0000_1010] = (
        10,
        [
            255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
            255, 255, 255, 255, 255, 255, 2, 255, // y coordinate
            4, 5, 6, 7, 8, 9, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9 99
    shuffle_patterns[0b0000_0000_0001_0010] = (
        11,
        [
            255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
            255, 255, 255, 255, 2, 255, 3, 255, // y coordinate
            5, 6, 7, 8, 9, 10, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9 999
    shuffle_patterns[0b0000_0000_0010_0010] = (
        12,
        [
            255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
            255, 255, 2, 255, 3, 255, 4, 255, // y coordinate
            6, 7, 8, 9, 10, 11, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9 9999
    shuffle_patterns[0b0000_0000_0100_0010] = (
        13,
        [
            255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
            2, 255, 3, 255, 4, 255, 5, 255, // y coordinate
            7, 8, 9, 10, 11, 12, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 99 9
    shuffle_patterns[0b0000_0000_0001_0100] = (
        11,
        [
            255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
            255, 255, 255, 255, 255, 255, 3, 255, // y coordinate
            5, 6, 7, 8, 9, 10, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 99 99
    shuffle_patterns[0b0000_0000_0010_0100] = (
        12,
        [
            255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
            255, 255, 255, 255, 3, 255, 4, 255, // y coordinate
            6, 7, 8, 9, 10, 11, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 99 999
    shuffle_patterns[0b0000_0000_0100_0100] = (
        13,
        [
            255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
            255, 255, 3, 255, 4, 255, 5, 255, // y coordinate
            7, 8, 9, 10, 11, 12, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 99 9999
    shuffle_patterns[0b0000_0000_1000_0100] = (
        14,
        [
            255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
            3, 255, 4, 255, 5, 255, 6, 255, // y coordinate
            8, 9, 10, 11, 12, 13, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 999 9
    shuffle_patterns[0b0000_0000_0010_1000] = (
        12,
        [
            255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
            255, 255, 255, 255, 255, 255, 4, 255, // y coordinate
            6, 7, 8, 9, 10, 11, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 999 99
    shuffle_patterns[0b0000_0000_0100_1000] = (
        13,
        [
            255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
            255, 255, 255, 255, 4, 255, 5, 255, // y coordinate
            7, 8, 9, 10, 11, 12, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 999 999
    shuffle_patterns[0b0000_0000_1000_1000] = (
        14,
        [
            255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
            255, 255, 4, 255, 5, 255, 6, 255, // y coordinate
            8, 9, 10, 11, 12, 13, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 999 9999
    shuffle_patterns[0b0000_0001_0000_1000] = (
        15,
        [
            255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
            4, 255, 5, 255, 6, 255, 7, 255, // y coordinate
            9, 10, 11, 12, 13, 14, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9999 9
    shuffle_patterns[0b0000_0000_0101_0000] = (
        13,
        [
            0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
            255, 255, 255, 255, 255, 255, 5, 255, // y coordinate
            7, 8, 9, 10, 11, 12, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9999 99
    shuffle_patterns[0b0000_0000_1001_0000] = (
        14,
        [
            0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
            255, 255, 255, 255, 5, 255, 6, 255, // y coordinate
            8, 9, 10, 11, 12, 13, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9999 999
    shuffle_patterns[0b0000_0001_0001_0000] = (
        15,
        [
            0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
            255, 255, 5, 255, 6, 255, 7, 255, // y coordinate
            9, 10, 11, 12, 13, 14, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // 9999 9999
    shuffle_patterns[0b0000_0010_0001_0000] = (
        16,
        [
            0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
            5, 255, 6, 255, 7, 255, 8, 255, // y coordinate
            10, 11, 12, 13, 14, 15, 255, 255, // rgb + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    shuffle_patterns
}

#[cfg(test)]
mod tests {
    use super::PARSER_LOOKAHEAD;
    use fearless_simd::{Level, dispatch};
    use rstest::rstest;

    use crate::{FearParser, Parser, SimpleFrameBuffer};

    fn simd_parse(buffer: *const u8) -> (u16, u16, u32, u8) {
        let level = Level::new();

        dispatch!(level, simd => super::simd_parse(simd, buffer))
    }

    #[rstest]
    #[case("", 0, 0, 0, 0)]
    #[case(" ", 0, 0, 0, 0)]
    #[case("1 2", 0, 0, 0, 0)]
    #[case("1 2 ", 1, 2, 10_066_329 /* invalid input produces garbage */, 3)]
    #[case("1 2 abcdef", 1, 2, 0xab_cdef, 3)]
    #[case("1234 5678 ", 1234, 5678, 10_066_329 /* invalid input produces garbage */, 9)]
    #[case("1234 5678 09afAF", 1234, 5678, 0x09_afaf, 9)]
    fn test_simd_parse(
        #[case] input: &str,
        #[case] expected_x: u16,
        #[case] expected_y: u16,
        #[case] expected_rgb: u32,
        #[case] expected_bytes_parsed: u8,
    ) {
        let mut buffer: Vec<u8> = input.as_bytes().to_vec();
        buffer.resize(32, 0);
        let (x, y, rgb, bytes_parsed) = simd_parse(buffer.as_ptr());
        assert_eq!(x, expected_x);
        assert_eq!(y, expected_y);
        assert_eq!(rgb, expected_rgb);
        assert_eq!(bytes_parsed, expected_bytes_parsed);
    }

    #[rstest]
    #[case("PX 1 2 19afAF")]
    #[case("PX 1 2 19afAF\nPX 1 2 19afAF\nPX 1 2 19afAF\nPX 1 2 19afAF\n")]
    fn e2e(#[case] input: &str) {
        use std::sync::Arc;

        let mut input = input.as_bytes().to_vec();
        input.extend([0; PARSER_LOOKAHEAD]);
        let mut response = vec![];
        let fb = Arc::new(SimpleFrameBuffer::new(10, 10));
        let mut parser = FearParser::new(fb);
        parser.parse(&input, &mut response);
    }
}
