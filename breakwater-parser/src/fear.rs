#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    _mm_add_epi8, _mm_and_si128, _mm_cmpgt_epi8, _mm_cvtsi128_si32, _mm_extract_epi32,
    _mm_load_si128, _mm_madd_epi16, _mm_maddubs_epi16, _mm_packus_epi16, _mm_set1_epi8,
    _mm_setr_epi8, _mm_setr_epi16, _mm_shuffle_epi8, _mm256_castsi256_si128, _mm256_cmpeq_epi8,
    _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
};
use std::sync::Arc;

use fearless_simd::{Level, Simd, SimdBase, SimdFrom, SimdMask, dispatch, u8x32};
use fearless_simd_macros::simd;

use crate::original::{HELP_PATTERN, PX_PATTERN};
use crate::{ALT_HELP_TEXT, FrameBuffer, HELP_TEXT, MAX_HELP_CALLS_PER_CONNECTION, Parser};

/// We work on 32 byte vectors
pub const PARSER_LOOKAHEAD: usize = 32;

// Longest possible space bitmask = "1234 1234 " => 10 chars
const SPACES_BITMASK_BITS: u32 = 10;
const SPACES_BITMASK_MASK: u32 = (1 << SPACES_BITMASK_BITS) - 1;

/// Shuffle index that makes `pshufb` produce a zero byte
const ZERO: u8 = 0x80;

#[derive(Clone, Copy)]
#[repr(C, align(32))]
struct ShufflePattern {
    /// Source byte for every output byte, see [`shuffle_patterns`] for the layout
    indices: [u8; 16],
    /// Whether the spaces bitmask belongs to a valid `x y rrggbb` command
    valid: bool,
}

static SHUFFLE_PATTERNS: [ShufflePattern; 1 << SPACES_BITMASK_BITS] = shuffle_patterns();

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

                let (x, y, rgb, valid, newline_pos) = dispatch!(self.simd_level, simd => simd_parse(simd, unsafe { buffer.as_ptr().add(i)}));

                // Branch on `valid` instead of folding it into the advance: a predicted branch keeps
                // the next `i` independent of the shuffle table load.
                if valid {
                    last_byte_parsed = i + newline_pos as usize;
                    i += newline_pos as usize + 1; // We can advance one byte more than normal as we use continue and therefore not get incremented at the end of the loop

                    // The alpha byte of `rgb` is always zero
                    self.fb.set(x as usize, y as usize, rgb, current_ts);
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

/// Parses `x y rrggbb` from the 32 bytes after `PX `.
///
/// Returns `(x, y, rgb, valid, newline_pos)`. `rgb` has the red channel in the lowest byte and a
/// zero alpha byte. `newline_pos` is 32 if there is no newline.
#[simd]
fn simd_parse<S: Simd>(simd: S, buffer: *const u8) -> (u32, u32, u32, bool, u8) {
    // SAFETY: The caller guarantees `PARSER_LOOKAHEAD` readable bytes
    let chars = unsafe { &*(buffer as *const [u8; 32]) };

    #[cfg(target_arch = "x86_64")]
    if let Some(avx2) = simd.level().as_avx2() {
        return simd_parse_avx2(avx2, chars);
    }

    simd_parse_portable(simd, chars)
}

#[cfg(target_arch = "x86_64")]
fearless_simd::kernel!(
    #[inline(always)]
    fn simd_parse_avx2(avx2: Avx2, chars: &[u8; 32]) -> (u32, u32, u32, bool, u8) {
        // SAFETY: `chars` is 32 bytes long
        let chars = unsafe { _mm256_loadu_si256(chars.as_ptr().cast()) };

        let spaces_bitmask =
            _mm256_movemask_epi8(_mm256_cmpeq_epi8(chars, _mm256_set1_epi8(b' ' as i8))) as u32;
        let newlines_bitmask =
            _mm256_movemask_epi8(_mm256_cmpeq_epi8(chars, _mm256_set1_epi8(b'\n' as i8))) as u32;

        // The command length comes straight from the newline position, so the caller's next `i`
        // doesn't have to wait for the table load below.
        let newline_pos = newlines_bitmask.trailing_zeros() as u8;

        let pattern = &SHUFFLE_PATTERNS[(spaces_bitmask & SPACES_BITMASK_MASK) as usize];
        // SAFETY: `ShufflePattern` is 32 byte aligned and starts with the 16 indices
        let indices = unsafe { _mm_load_si128(pattern.indices.as_ptr().cast()) };
        // All indices are < 16 or `ZERO`, so the lower 16 bytes are all we need
        let shuffled = _mm_shuffle_epi8(_mm256_castsi256_si128(chars), indices);

        // `0-9` -> 0-9, and the low nibble of `a-f` and `A-F` is 1-6. Zeroed bytes stay 0.
        let low_nibbles = _mm_and_si128(shuffled, _mm_set1_epi8(0x0f));

        // x and y: `(d0 * 10 + d1) * 100 + (d2 * 10 + d3)`, in the two upper i32 lanes
        let pairs = _mm_maddubs_epi16(
            low_nibbles,
            _mm_setr_epi8(0, 0, 0, 0, 0, 0, 0, 0, 10, 1, 10, 1, 10, 1, 10, 1),
        );
        let coordinates = _mm_madd_epi16(pairs, _mm_setr_epi16(0, 0, 0, 0, 100, 1, 100, 1));
        let x = _mm_extract_epi32::<2>(coordinates) as u32;
        let y = _mm_extract_epi32::<3>(coordinates) as u32;

        // rgb: letters (> '@') need 9 added to their low nibble to become 10-15
        let letters = _mm_cmpgt_epi8(shuffled, _mm_set1_epi8(0x40));
        let nibbles = _mm_add_epi8(low_nibbles, _mm_and_si128(letters, _mm_set1_epi8(9)));
        // `high * 16 + low` for the three channels in the lower i16 lanes, lane 3 is 0 (alpha)
        let channels = _mm_maddubs_epi16(
            nibbles,
            _mm_setr_epi8(16, 1, 16, 1, 16, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
        );
        let rgb = _mm_cvtsi128_si32(_mm_packus_epi16(channels, channels)) as u32;

        (x, y, rgb, pattern.valid, newline_pos)
    }
);

/// Slow path for SIMD levels without AVX2, uses the same table as [`simd_parse_avx2`].
#[inline(always)]
fn simd_parse_portable<S: Simd>(simd: S, chars: &[u8; 32]) -> (u32, u32, u32, bool, u8) {
    let vector = u8x32::simd_from(simd, *chars);
    let spaces_bitmask = vector.simd_eq(u8x32::splat(simd, b' ')).to_bitmask() as u32;
    let newlines_bitmask = vector.simd_eq(u8x32::splat(simd, b'\n')).to_bitmask() as u32;
    let newline_pos = newlines_bitmask.trailing_zeros() as u8;

    let pattern = &SHUFFLE_PATTERNS[(spaces_bitmask & SPACES_BITMASK_MASK) as usize];
    let shuffled = pattern
        .indices
        .map(|index| if index < 16 { chars[index as usize] } else { 0 });

    let decimal = |digits: &[u8]| {
        digits
            .iter()
            .fold(0, |acc, digit| acc * 10 + u32::from(digit & 0x0f))
    };
    let nibble = |char: u8| (char & 0x0f) + u8::from(char > 0x40) * 9;
    let channel = |i: usize| (nibble(shuffled[i]) << 4) | nibble(shuffled[i + 1]);
    let rgb = u32::from_le_bytes([channel(0), channel(2), channel(4), 0]);

    (
        decimal(&shuffled[8..12]),
        decimal(&shuffled[12..16]),
        rgb,
        pattern.valid,
        newline_pos,
    )
}

/// Builds the shuffle patterns for all combinations of 1-4 digit coordinates, indexed by the
/// bitmask of the two spaces after them.
///
/// Output layout: bytes 0-5 are `rrggbb`, bytes 6-7 zero, bytes 8-11 the x digits and bytes 12-15
/// the y digits. Coordinates are right-aligned and padded with zero bytes, which act as `0`
/// digits.
#[expect(clippy::large_stack_arrays, reason = "only evaluated at compile time")]
const fn shuffle_patterns() -> [ShufflePattern; 1 << SPACES_BITMASK_BITS] {
    let mut patterns = [ShufflePattern {
        indices: [ZERO; 16],
        valid: false,
    }; 1 << SPACES_BITMASK_BITS];

    let mut x_len = 1;
    while x_len <= 4 {
        let mut y_len = 1;
        while y_len <= 4 {
            let y_start = x_len + 1;
            let rgb_start = y_start + y_len + 1;
            let mut indices = [ZERO; 16];

            let mut i = 0;
            while i < 6 {
                indices[i] = (rgb_start + i) as u8;
                i += 1;
            }
            let mut i = 0;
            while i < x_len {
                indices[12 - x_len + i] = i as u8;
                i += 1;
            }
            let mut i = 0;
            while i < y_len {
                indices[16 - y_len + i] = (y_start + i) as u8;
                i += 1;
            }

            patterns[(1 << x_len) | (1 << (y_start + y_len))] = ShufflePattern {
                indices,
                valid: true,
            };
            y_len += 1;
        }
        x_len += 1;
    }

    patterns
}

#[cfg(test)]
mod tests {
    use super::PARSER_LOOKAHEAD;
    use fearless_simd::{Level, dispatch};
    use rstest::rstest;

    use crate::{FearParser, Parser, SimpleFrameBuffer};

    fn simd_parse(buffer: *const u8) -> (u16, u16, u32, u8) {
        let level = Level::new();

        let (x, y, rgb, valid, newline_pos) =
            dispatch!(level, simd => super::simd_parse(simd, buffer));
        (x as u16, y as u16, rgb, if valid { newline_pos } else { 0 })
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
