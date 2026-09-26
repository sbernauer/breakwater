#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    _mm_add_epi8, _mm_and_si128, _mm_cmpgt_epi8, _mm_cvtsi128_si32, _mm_extract_epi32,
    _mm_load_si128, _mm_madd_epi16, _mm_maddubs_epi16, _mm_packus_epi16, _mm_set1_epi8,
    _mm_setr_epi8, _mm_setr_epi16, _mm_shuffle_epi8, _mm256_add_epi16, _mm256_castsi256_si128,
    _mm256_cmpeq_epi8, _mm256_cvtepu8_epi16, _mm256_loadu_si256, _mm256_movemask_epi8,
    _mm256_set1_epi8, _mm256_set1_epi16, _mm256_storeu_si256, _mm512_castsi512_si128,
    _mm512_loadu_si512, _mm512_maskz_compress_epi8,
};
use std::sync::Arc;

use fearless_simd::{Level, Simd, SimdBase, SimdFrom, SimdMask, dispatch, u8x32, u8x64};
use fearless_simd_macros::simd;

use crate::original::{HELP_PATTERN, PX_PATTERN};
use crate::{ALT_HELP_TEXT, FrameBuffer, HELP_TEXT, MAX_HELP_CALLS_PER_CONNECTION, Parser};

/// Stage 1 reads 64 byte blocks, a command reads 3 + 32 bytes from the start of its line
pub const PARSER_LOOKAHEAD: usize = 64;

/// Stage 2 re-reads the input stage 1 just scanned, so we alternate between the stages on chunks
/// that fit into L1
const CHUNK_SIZE: usize = 16 * 1024;

/// Stage 1 writes up to this many newline offsets per 64 byte block, no matter how many it found
const MAX_OFFSETS_PER_BLOCK: usize = 16;

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
    /// Stage 1 output: offsets of the newlines in the current chunk, relative to the chunk start
    newline_offsets: Box<[u16]>,
}

impl<FB: FrameBuffer> FearParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self {
            help_count: 0,
            fb,
            simd_level: Level::new(),
            // At most one newline per byte, plus the unconditional writes of the last block
            newline_offsets: vec![0; CHUNK_SIZE + MAX_OFFSETS_PER_BLOCK].into_boxed_slice(),
        }
    }
}

impl<FB: FrameBuffer> Parser for FearParser<FB> {
    // Inlined into the caller the loops get less registers, so loop invariants get spilled
    #[inline(never)]
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize {
        let level = self.simd_level;
        dispatch!(level, simd => parse_simd(simd, self, buffer, response))
    }

    fn parser_lookahead(&self) -> usize {
        PARSER_LOOKAHEAD
    }
}

/// Parses in two stages, so that the start of a command never depends on parsing the previous one:
///
/// 1. Find all newlines in a chunk, in fixed 64 byte steps.
/// 2. Parse every line ending at one of those newlines. The lines are independent of each other,
///    so the CPU can work on several of them at once.
///
/// Commands are only recognized at the start of a line.
#[simd]
#[allow(clippy::too_many_lines)]
fn parse_simd<S: Simd, FB: FrameBuffer>(
    simd: S,
    parser: &mut FearParser<FB>,
    buffer: &[u8],
    response: &mut Vec<u8>,
) -> usize {
    // As this is a potentially(?) expensive operation we only call it one in this parsing loop
    // All the pixels likely where in the same TCP packets (+- 1/2 or so) it doesn't matter after all
    // Encode the timestamp exactly once here, not per pixel: it's constant for the whole parse
    // call, so computing it per write would just waste throughput on the hot path.
    let current_ts = parser.fb.current_ts();

    let mut last_byte_parsed = 0;

    // Only lines ending before this are parsed, the lookahead guarantees all reads stay in bounds
    let loop_end = buffer.len().saturating_sub(PARSER_LOOKAHEAD);
    let newline_chars = u8x64::splat(simd, b'\n');
    let mut line_start = 0;

    let mut chunk_start = 0;
    while chunk_start < loop_end {
        let chunk_end = (chunk_start + CHUNK_SIZE).min(loop_end);

        // Stage 1: collect the offsets of all newlines in the chunk
        let mut newline_count = 0;
        let mut block = chunk_start;
        while block < chunk_end {
            // SAFETY: `block < loop_end`, so the lookahead guarantees 64 readable bytes
            let chars = unsafe { (buffer.as_ptr().add(block) as *const [u8; 64]).read_unaligned() };
            let mut newlines = u8x64::simd_from(simd, chars)
                .simd_eq(newline_chars)
                .to_bitmask();
            let remaining = chunk_end - block;
            if remaining < 64 {
                newlines &= (1 << remaining) - 1;
            }
            let block_count = newlines.count_ones() as usize;
            let block_offset = (block - chunk_start) as u16;

            // Always write a fixed number of offsets, so the loop doesn't branch on the number of
            // newlines. The ones past `block_count` are garbage, the next block overwrites them.
            let offsets: &mut [u16; MAX_OFFSETS_PER_BLOCK] = (&mut parser.newline_offsets
                [newline_count..newline_count + MAX_OFFSETS_PER_BLOCK])
                .try_into()
                .unwrap();
            let written = write_newline_offsets(simd, newlines, block_offset, offsets);
            if block_count > written {
                // Only garbage input has this many newlines per block
                let mut index = newline_count;
                while newlines != 0 {
                    parser.newline_offsets[index] = block_offset + newlines.trailing_zeros() as u16;
                    newlines &= newlines - 1;
                    index += 1;
                }
            }

            newline_count += block_count;
            block += 64;
        }

        // Stage 2: parse the lines
        for &offset in &parser.newline_offsets[..newline_count] {
            let newline = chunk_start + offset as usize;
            let start = line_start;
            line_start = newline + 1;

            // SAFETY: `start <= newline < loop_end`, so the lookahead guarantees 8 readable bytes
            let current_command =
                unsafe { (buffer.as_ptr().add(start) as *const u64).read_unaligned() };
            if current_command & 0x00ff_ffff == PX_PATTERN {
                // SAFETY: `start + 3 + 32 <= loop_end + 35`, which the lookahead covers
                let (x, y, rgb, valid) =
                    simd_parse(simd, unsafe { buffer.as_ptr().add(start + 3) });

                if valid {
                    // The alpha byte of `rgb` is always zero
                    parser.fb.set(x as usize, y as usize, rgb, current_ts);
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
            } else if current_command & 0xffff_ffff == HELP_PATTERN {
                match parser.help_count {
                    0..MAX_HELP_CALLS_PER_CONNECTION => {
                        response.extend_from_slice(HELP_TEXT);
                        parser.help_count += 1;
                    }
                    MAX_HELP_CALLS_PER_CONNECTION => {
                        response.extend_from_slice(ALT_HELP_TEXT);
                        parser.help_count += 1;
                    }
                    _ => {
                        // The client has requested the help to often, let's just ignore it
                    }
                }
            }
        }

        if newline_count > 0 {
            last_byte_parsed = line_start - 1;
        }
        chunk_start = chunk_end;
    }

    last_byte_parsed
    // last_byte_parsed.saturating_sub(1)
}

/// Writes `block_offset` plus the positions of the lowest set bits of `newlines` to `offsets`,
/// followed by garbage. Returns how many offsets are written, which doesn't depend on `newlines`.
#[inline(always)]
fn write_newline_offsets<S: Simd>(
    simd: S,
    newlines: u64,
    block_offset: u16,
    offsets: &mut [u16; MAX_OFFSETS_PER_BLOCK],
) -> usize {
    #[cfg(target_arch = "x86_64")]
    if let Some(avx512) = simd.level().as_avx512() {
        newline_offsets_avx512(avx512, newlines, block_offset, offsets);
        return MAX_OFFSETS_PER_BLOCK;
    }

    let mut newlines = newlines;
    for offset in &mut offsets[..8] {
        *offset = block_offset + newlines.trailing_zeros() as u16;
        newlines &= newlines.wrapping_sub(1);
    }
    8
}

#[cfg(target_arch = "x86_64")]
static BYTE_INDICES: [u8; 64] = {
    let mut indices = [0; 64];
    let mut i = 0;
    while i < 64 {
        indices[i] = i as u8;
        i += 1;
    }
    indices
};

#[cfg(target_arch = "x86_64")]
fearless_simd::kernel!(
    #[inline(always)]
    fn newline_offsets_avx512(
        avx512: Avx512,
        newlines: u64,
        block_offset: u16,
        offsets: &mut [u16; MAX_OFFSETS_PER_BLOCK],
    ) {
        // SAFETY: `BYTE_INDICES` is 64 bytes long
        let indices = unsafe { _mm512_loadu_si512(BYTE_INDICES.as_ptr().cast()) };
        // Packs the positions of all newlines into the lowest bytes
        let positions = _mm512_maskz_compress_epi8(newlines, indices);
        let positions = _mm256_add_epi16(
            _mm256_cvtepu8_epi16(_mm512_castsi512_si128(positions)),
            _mm256_set1_epi16(block_offset as i16),
        );
        // SAFETY: `offsets` holds 16 u16s, which is 32 bytes
        unsafe { _mm256_storeu_si256(offsets.as_mut_ptr().cast(), positions) };
    }
);

/// Parses `x y rrggbb` from the 32 bytes after `PX `.
///
/// Returns `(x, y, rgb, valid)`. `rgb` has the red channel in the lowest byte and a zero alpha
/// byte.
#[simd]
fn simd_parse<S: Simd>(simd: S, buffer: *const u8) -> (u32, u32, u32, bool) {
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
    fn simd_parse_avx2(avx2: Avx2, chars: &[u8; 32]) -> (u32, u32, u32, bool) {
        // SAFETY: `chars` is 32 bytes long
        let chars = unsafe { _mm256_loadu_si256(chars.as_ptr().cast()) };

        let spaces_bitmask =
            _mm256_movemask_epi8(_mm256_cmpeq_epi8(chars, _mm256_set1_epi8(b' ' as i8))) as u32;

        let pattern = &SHUFFLE_PATTERNS[(spaces_bitmask & SPACES_BITMASK_MASK) as usize];
        // SAFETY: `ShufflePattern` is 32 byte aligned and starts with the 16 indices
        let indices = unsafe { _mm_load_si128(pattern.indices.as_ptr().cast()) };
        // All indices are < 16 or `ZERO`, so the lower 16 bytes are all we need
        let shuffled = _mm_shuffle_epi8(_mm256_castsi256_si128(chars), indices);

        // `0-9` -> 0-9, and the low nibble of `a-f` and `A-F` is 1-6. Letters (> '@') need 9 added
        // to become 10-15. Zeroed bytes stay 0.
        let low_nibbles = _mm_and_si128(shuffled, _mm_set1_epi8(0x0f));
        let letters = _mm_cmpgt_epi8(shuffled, _mm_set1_epi8(0x40));
        let nibbles = _mm_add_epi8(low_nibbles, _mm_and_si128(letters, _mm_set1_epi8(9)));

        // i16 lanes 0-2: `high * 16 + low` for the three channels, lane 3 is 0 (alpha).
        // i16 lanes 4-7: `d0 * 10 + d1` for the digit pairs of x and y.
        let pairs = _mm_maddubs_epi16(
            nibbles,
            _mm_setr_epi8(16, 1, 16, 1, 16, 1, 0, 0, 10, 1, 10, 1, 10, 1, 10, 1),
        );

        // x and y: `(d0 * 10 + d1) * 100 + (d2 * 10 + d3)`, in the two upper i32 lanes
        let coordinates = _mm_madd_epi16(pairs, _mm_setr_epi16(0, 0, 0, 0, 100, 1, 100, 1));
        let x = _mm_extract_epi32::<2>(coordinates) as u32;
        let y = _mm_extract_epi32::<3>(coordinates) as u32;

        // The channels fit into a byte, the saturated coordinate pairs end up in bytes 4-7
        let rgb = _mm_cvtsi128_si32(_mm_packus_epi16(pairs, pairs)) as u32;

        (x, y, rgb, pattern.valid)
    }
);

/// Slow path for SIMD levels without AVX2, uses the same table as [`simd_parse_avx2`].
#[inline(always)]
fn simd_parse_portable<S: Simd>(simd: S, chars: &[u8; 32]) -> (u32, u32, u32, bool) {
    let vector = u8x32::simd_from(simd, *chars);
    let spaces_bitmask = vector.simd_eq(u8x32::splat(simd, b' ')).to_bitmask() as u32;

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

        let (x, y, rgb, valid) = dispatch!(level, simd => super::simd_parse(simd, buffer));
        // SAFETY: The tests pass 32 byte buffers
        let chars = unsafe { &*(buffer as *const [u8; 32]) };
        let newline_pos = chars.iter().position(|&c| c == b'\n').unwrap_or(32) as u8;
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
