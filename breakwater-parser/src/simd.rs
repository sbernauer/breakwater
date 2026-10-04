#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m128i, _mm_add_epi8, _mm_add_epi32, _mm_and_si128, _mm_cmpgt_epi8, _mm_cvtsi128_si32,
    _mm_extract_epi32, _mm_load_si128, _mm_madd_epi16, _mm_maddubs_epi16, _mm_packus_epi16,
    _mm_set1_epi8, _mm_setr_epi8, _mm_setr_epi16, _mm_shuffle_epi8, _mm256_add_epi16,
    _mm256_castsi256_si128, _mm256_cmpeq_epi8, _mm256_cvtepu8_epi16, _mm256_loadu_si256,
    _mm256_movemask_epi8, _mm256_set1_epi8, _mm256_set1_epi16, _mm256_storeu_si256,
    _mm512_castsi512_si128, _mm512_loadu_si512, _mm512_maskz_compress_epi8,
};
use std::{io::Write, sync::Arc};

use fearless_simd::{Level, Simd, SimdBase, SimdFrom, SimdMask, dispatch, u8x32, u8x64, u32x4};
use fearless_simd_macros::simd;

use crate::original::{HELP_PATTERN, OFFSET_PATTERN, PX_PATTERN, SIZE_PATTERN};
use crate::{ALT_HELP_TEXT, FrameBuffer, HELP_TEXT, MAX_HELP_CALLS_PER_CONNECTION, Parser};

/// Stage 1 reads 64 byte blocks, a command reads 3 + 32 bytes from the start of its line
pub const PARSER_LOOKAHEAD: usize = 64;

/// A feature this build enables, but SimdParser can't support. It works on lines, so the binary
/// commands are out: their payload can contain newlines and doesn't end with one.
pub(crate) const UNSUPPORTED_ENABLED_FEATURE: Option<&str> = if cfg!(feature = "binary-set-pixel") {
    Some("binary-set-pixel")
} else if cfg!(feature = "binary-sync-pixels") {
    Some("binary-sync-pixels")
} else {
    None
};

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
    /// Length of the `x y rrggbb` command after `PX ` (without the newline). 0 if the spaces
    /// bitmask doesn't belong to one.
    len: u8,
}

static SHUFFLE_PATTERNS: [ShufflePattern; 1 << SPACES_BITMASK_BITS] = shuffle_patterns();

pub struct SimdParser<FB: FrameBuffer> {
    /// `[0, 0, x, y]` of the last `OFFSET x y`, which is added to the coordinates of all following
    /// `PX` commands of the connection. The layout matches the lanes of the coordinates in
    /// [`simd_parse`].
    offsets: [u32; 4],
    /// How often the client requested the help on this connection. It is tracked per connection.
    help_count: u8,
    fb: Arc<FB>,
    simd_level: Level,
    /// Stage 1 output: offsets of the newlines in the current chunk, relative to the chunk start
    newline_offsets: Box<[u16]>,
}

impl<FB: FrameBuffer> SimdParser<FB> {
    pub fn new(fb: Arc<FB>) -> Self {
        Self {
            offsets: [0; 4],
            help_count: 0,
            fb,
            simd_level: Level::new(),
            // At most one newline per byte, plus the unconditional writes of the last block
            newline_offsets: vec![0; CHUNK_SIZE + MAX_OFFSETS_PER_BLOCK].into_boxed_slice(),
        }
    }
}

impl<FB: FrameBuffer> Parser for SimdParser<FB> {
    // Keep the parse loops in their own function. Inlined into the caller (e.g. criterion's
    // `Bencher::iter` closure) they compete with the caller's code for registers: loop invariants
    // got spilled to the stack and the SIMD constants stayed memory operands, reloaded for every
    // line. That cost ~19% (ordered benchmark 5.97 instead of 7.38 ms when this was added).
    // OriginalParser in contrast gets slower with `#[inline(never)]` (5.34 -> 5.82 ms).
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
    parser: &mut SimdParser<FB>,
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
    // Keep the offsets in a vector register, stage 2 already uses all general purpose registers. As
    // two integers one of them got spilled to the stack, which cost 12% on the ordered benchmark.
    // To stay a vector, the loop must never build it from scalars (LLVM then keeps the scalars
    // and inserts them on every use), see `parse_offset`.
    let mut offsets = u32x4::simd_from(simd, parser.offsets);

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
                let (x, y, rgb, len) =
                    simd_parse(simd, unsafe { buffer.as_ptr().add(start + 3) }, offsets);

                // `PX ` contains no newline, so the line is at least that long
                let line_len = newline - (start + 3);
                let len = usize::from(len);
                // `x y rrggbb`, and without the `alpha` feature also `x y rrggbbaa` with the alpha
                // channel ignored. Checking the length also ensures the spaces that picked the
                // pattern are part of this line.
                if len != 0
                    && (line_len == len || (!cfg!(feature = "alpha") && line_len == len + 2))
                {
                    // The alpha byte of `rgb` is always zero
                    parser.fb.set(x as usize, y as usize, rgb, current_ts);
                } else {
                    let [_, _, x_offset, y_offset] = <[u32; 4]>::from(offsets);
                    parse_px_slow_path(
                        &*parser.fb,
                        &buffer[start + 3..newline],
                        (x_offset as usize, y_offset as usize),
                        current_ts,
                        response,
                    );
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
            } else if current_command & 0x00ff_ffff_ffff_ffff == OFFSET_PATTERN {
                // `OFFSET ` contains no newline, so the line is at least that long
                if let Some(new_offsets) = parse_offset(simd, &buffer[start + 7..newline]) {
                    offsets = new_offsets;
                }
            } else {
                parse_text_command(
                    current_command,
                    &*parser.fb,
                    &mut parser.help_count,
                    response,
                );
            }
        }

        if newline_count > 0 {
            last_byte_parsed = line_start - 1;
        }
        chunk_start = chunk_end;
    }

    // The last line might miss its newline, e.g. a client sending `SIZE` and waiting for the
    // answer. Only take the text commands from it, a `PX` command could still be incomplete.
    if loop_end.saturating_sub(line_start) >= 4 {
        // SAFETY: `line_start <= loop_end`, so the lookahead guarantees 8 readable bytes
        let current_command =
            unsafe { (buffer.as_ptr().add(line_start) as *const u64).read_unaligned() };
        if parse_text_command(
            current_command,
            &*parser.fb,
            &mut parser.help_count,
            response,
        ) {
            last_byte_parsed = line_start + 3;
        }
    }

    parser.offsets = offsets.into();

    last_byte_parsed
    // last_byte_parsed.saturating_sub(1)
}

/// Handles `HELP` and `SIZE`, `command` holds the first 8 bytes of the line. Returns whether it was
/// one of them.
// Keep this out of the stage 2 loop, which only needs to be fast for `PX`. Inlined, the formatting
// code made the loop 27% slower on the ordered and 14% on the unordered benchmark, although they
// don't contain a single text command: the bigger loop gets worse register allocation.
#[cold]
#[inline(never)]
fn parse_text_command<FB: FrameBuffer>(
    command: u64,
    fb: &FB,
    help_count: &mut u8,
    response: &mut Vec<u8>,
) -> bool {
    match command & 0xffff_ffff {
        HELP_PATTERN => {
            match *help_count {
                0..MAX_HELP_CALLS_PER_CONNECTION => {
                    response.extend_from_slice(HELP_TEXT);
                    *help_count += 1;
                }
                MAX_HELP_CALLS_PER_CONNECTION => {
                    response.extend_from_slice(ALT_HELP_TEXT);
                    *help_count += 1;
                }
                _ => {
                    // The client has requested the help to often, let's just ignore it
                }
            }
            true
        }
        SIZE_PATTERN => {
            writeln!(response, "SIZE {} {}", fb.get_width(), fb.get_height())
                .expect("writing to a Vec never fails");
            true
        }
        _ => false,
    }
}

/// Parses the `x y` of `OFFSET x y` into `[0, 0, x, y]`, `line` is everything between `OFFSET `
/// and the newline
// Out of the stage 2 loop for the same reason as `parse_text_command`. It also builds the vector,
// so that the loop only ever sees the offsets as a vector.
#[cold]
#[inline(never)]
fn parse_offset<S: Simd>(simd: S, line: &[u8]) -> Option<u32x4<S>> {
    let mut parts = line.split(|&char| char == b' ');
    let (x, y) = parse_coordinates(&mut parts)?;
    // Both have at most 4 digits
    parts
        .next()
        .is_none()
        .then(|| u32x4::simd_from(simd, [0, 0, x as u32, y as u32]))
}

/// Handles the `PX` commands the fast path doesn't, `line` is everything between `PX ` and the
/// newline: reading a pixel (`PX x y`), setting a gray one (`PX x y gg`) and with the `alpha`
/// feature blending one (`PX x y rrggbbaa`).
// Out of the stage 2 loop for the same reason as `parse_text_command`
#[cold]
#[inline(never)]
fn parse_px_slow_path<FB: FrameBuffer>(
    fb: &FB,
    line: &[u8],
    (x_offset, y_offset): (usize, usize),
    ts: FB::Timestamp,
    response: &mut Vec<u8>,
) {
    let mut parts = line.split(|&char| char == b' ');
    let Some((x, y)) = parse_coordinates(&mut parts) else {
        return;
    };

    match (parts.next(), parts.next()) {
        (None, _) => {
            if let Some(rgb) = fb.get(x + x_offset, y + y_offset) {
                // The client gets its coordinates back, without the offset. The framebuffer has
                // the red channel in the lowest byte, this prints `rrggbb`.
                writeln!(response, "PX {x} {y} {:06x}", rgb.to_be() >> 8)
                    .expect("writing to a Vec never fails");
            }
        }
        (Some(&[high, low]), None) => {
            if let (Some(high), Some(low)) = (hex_digit(high), hex_digit(low)) {
                let gray = (high << 4) | low;
                fb.set(x + x_offset, y + y_offset, gray * 0x01_0101, ts);
            }
        }
        #[cfg(feature = "alpha")]
        (Some(color), None) if color.len() == 8 => {
            if let Some(rgba) = parse_rgba(color) {
                blend_pixel(fb, x + x_offset, y + y_offset, rgba, ts);
            }
        }
        _ => {}
    }
}

/// Parses the next two space separated parts as coordinates
fn parse_coordinates<'a>(parts: &mut impl Iterator<Item = &'a [u8]>) -> Option<(usize, usize)> {
    Some((
        parts.next().and_then(parse_coordinate)?,
        parts.next().and_then(parse_coordinate)?,
    ))
}

fn hex_digit(char: u8) -> Option<u32> {
    char::from(char).to_digit(16)
}

/// Parses `rrggbbaa`, the red channel ends up in the lowest byte
#[cfg(feature = "alpha")]
fn parse_rgba(color: &[u8]) -> Option<u32> {
    let mut bytes = [0; 4];
    for (byte, &[high, low]) in bytes.iter_mut().zip(color.as_chunks::<2>().0) {
        *byte = ((hex_digit(high)? << 4) | hex_digit(low)?) as u8;
    }
    Some(u32::from_le_bytes(bytes))
}

/// Blends `rgba` over the current pixel, with exactly the formula of `OriginalParser` so both stay
/// comparable. That includes its bug of reading the channels of the current pixel one byte off
/// (`>> 24/16/8` instead of `>> 16/8/0`).
#[cfg(feature = "alpha")]
fn blend_pixel<FB: FrameBuffer>(fb: &FB, x: usize, y: usize, rgba: u32, ts: FB::Timestamp) {
    let alpha = (rgba >> 24) & 0xff;
    let Some(current) = fb.get(x, y) else {
        return;
    };
    if alpha == 0 {
        return;
    }

    let alpha_comp = 0xff - alpha;
    let blend = |shift: u32| {
        let current = (current >> (shift + 8)) & 0xff;
        let new = (rgba >> shift) & 0xff;
        ((current * alpha_comp + new * alpha) / 0xff) << shift
    };
    fb.set(x, y, blend(16) | blend(8) | blend(0), ts);
}

fn parse_coordinate(digits: &[u8]) -> Option<usize> {
    if !(1..=4).contains(&digits.len()) {
        return None;
    }

    digits.iter().try_fold(0, |acc, digit| {
        digit
            .is_ascii_digit()
            .then(|| acc * 10 + usize::from(digit - b'0'))
    })
}

/// Writes `block_offset` plus the positions of the lowest set bits of `newlines` to `offsets`,
/// followed by garbage. Returns how many offsets are written, which doesn't depend on `newlines`.
#[inline(always)]
fn write_newline_offsets<S: Simd>(
    #[cfg_attr(
        not(target_arch = "x86_64"),
        expect(
            unused_variables,
            reason = "Only the AVX-512 path on x86_64 needs the token"
        )
    )]
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
/// Returns `(x, y, rgb, len)`, see [`ShufflePattern::len`] for `len`. The offsets in lanes 2 and 3
/// of `offsets` are added to x and y. `rgb` has the red channel in the lowest byte and a zero alpha
/// byte.
#[simd]
fn simd_parse<S: Simd>(simd: S, buffer: *const u8, offsets: u32x4<S>) -> (u32, u32, u32, u8) {
    // SAFETY: The caller guarantees `PARSER_LOOKAHEAD` readable bytes
    let chars = unsafe { &*(buffer as *const [u8; 32]) };

    #[cfg(target_arch = "x86_64")]
    if let Some(avx2) = simd.level().as_avx2() {
        return simd_parse_avx2(avx2, chars, offsets.into());
    }

    simd_parse_portable(simd, chars, offsets)
}

#[cfg(target_arch = "x86_64")]
fearless_simd::kernel!(
    #[inline(always)]
    fn simd_parse_avx2(avx2: Avx2, chars: &[u8; 32], offsets: __m128i) -> (u32, u32, u32, u8) {
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

        // x and y: `(d0 * 10 + d1) * 100 + (d2 * 10 + d3)` plus the offset, in the two upper i32
        // lanes
        let coordinates = _mm_add_epi32(
            _mm_madd_epi16(pairs, _mm_setr_epi16(0, 0, 0, 0, 100, 1, 100, 1)),
            offsets,
        );
        let x = _mm_extract_epi32::<2>(coordinates) as u32;
        let y = _mm_extract_epi32::<3>(coordinates) as u32;

        // The channels fit into a byte, the saturated coordinate pairs end up in bytes 4-7
        let rgb = _mm_cvtsi128_si32(_mm_packus_epi16(pairs, pairs)) as u32;

        (x, y, rgb, pattern.len)
    }
);

/// Slow path for SIMD levels without AVX2, uses the same table as [`simd_parse_avx2`].
#[inline(always)]
fn simd_parse_portable<S: Simd>(
    simd: S,
    chars: &[u8; 32],
    offsets: u32x4<S>,
) -> (u32, u32, u32, u8) {
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

    let [_, _, x_offset, y_offset] = <[u32; 4]>::from(offsets);
    (
        decimal(&shuffled[8..12]) + x_offset,
        decimal(&shuffled[12..16]) + y_offset,
        rgb,
        pattern.len,
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
        len: 0,
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
                len: (rgb_start + 6) as u8,
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
    use fearless_simd::{Level, SimdFrom, dispatch, u32x4};
    use rstest::rstest;

    use crate::{FrameBuffer, Parser, SimdParser, SimpleFrameBuffer};

    /// Runs [`super::simd_parse`] on `input` padded to 32 bytes, and checks that the portable
    /// fallback agrees
    fn simd_parse(input: &str, offsets: [u32; 4]) -> (u32, u32, u32, u8) {
        let mut buffer = input.as_bytes().to_vec();
        buffer.resize(32, 0);
        let level = Level::new();

        let result = dispatch!(level, simd => super::simd_parse(
            simd,
            buffer.as_ptr(),
            u32x4::simd_from(simd, offsets)
        ));

        #[cfg(target_arch = "x86_64")]
        if let Some(sse4_2) = level.as_sse4_2() {
            let chars: &[u8; 32] = buffer.as_slice().try_into().unwrap();
            let portable =
                super::simd_parse_portable(sse4_2, chars, u32x4::simd_from(sse4_2, offsets));
            assert_eq!(portable, result, "portable fallback differs for {input:?}");
        }

        result
    }

    #[rstest]
    // The spaces don't match any command, so everything is 0
    #[case("", 0, 0, 0, 0)]
    #[case(" ", 0, 0, 0, 0)]
    #[case("1 2", 0, 0, 0, 0)]
    #[case("1 2 ", 1, 2, 0 /* invalid input produces garbage */, 10)]
    #[case("1 2 abcdef", 1, 2, 0x00ef_cdab, 10)]
    #[case("12 345 abcdef", 12, 345, 0x00ef_cdab, 13)]
    #[case("1234 5678 ", 1234, 5678, 0 /* invalid input produces garbage */, 16)]
    #[case("1234 5678 09afAF", 1234, 5678, 0x00af_af09, 16)]
    // Only the first 6 hex digits matter, the length tells the caller about the alpha channel
    #[case("1 2 abcdef42", 1, 2, 0x00ef_cdab, 10)]
    fn test_simd_parse(
        #[case] input: &str,
        #[case] expected_x: u32,
        #[case] expected_y: u32,
        #[case] expected_rgb: u32,
        #[case] expected_len: u8,
    ) {
        assert_eq!(
            simd_parse(input, [0; 4]),
            (expected_x, expected_y, expected_rgb, expected_len)
        );
    }

    #[test]
    fn simd_parse_adds_offsets() {
        assert_eq!(
            simd_parse("1 2 abcdef", [0, 0, 10, 20]),
            (11, 22, 0x00ef_cdab, 10)
        );
    }

    #[cfg(feature = "alpha")]
    #[test]
    fn alpha_blends_like_original_parser() {
        use std::sync::Arc;

        use crate::OriginalParser;

        let mut input = b"PX 0 0 abcdef\nPX 0 0 12345680\nPX 1 0 ffffff00\nPX 2 0 ffffff88\n\
            PX 3 0 102030ff\nPX 3 0 abcdef11\nOFFSET 1 1\nPX 3 3 abcdef80\nPX 9999 0 abcdef80\n"
            .to_vec();
        input.extend([0; PARSER_LOOKAHEAD]);
        let fb_original = Arc::new(SimpleFrameBuffer::new(10, 10));
        let fb_simd = Arc::new(SimpleFrameBuffer::new(10, 10));
        OriginalParser::new(fb_original.clone()).parse(&input, &mut vec![]);
        SimdParser::new(fb_simd.clone()).parse(&input, &mut vec![]);

        for y in 0..10 {
            for x in 0..10 {
                assert_eq!(fb_original.get(x, y), fb_simd.get(x, y), "pixel {x} {y}");
            }
        }
    }

    #[test]
    fn read_followed_by_longer_command() {
        use std::sync::Arc;

        // The space bitmask of `10 0\nPX 100 ` looks like `x y rrggbb` with a 4 digit y (`0\nPX`),
        // only the line length tells it's a read
        let mut input = b"PX 10 0\nPX 100 3 ffffff\n".to_vec();
        input.extend([0; PARSER_LOOKAHEAD]);
        let fb = Arc::new(SimpleFrameBuffer::new(1920, 1080));
        let mut response = vec![];
        SimdParser::new(fb.clone()).parse(&input, &mut response);

        assert_eq!(String::from_utf8_lossy(&response), "PX 10 0 000000\n");
        // Where the misparsed y (`0\nPX` = 0, 10, 0, 8) would end up
        assert_eq!(fb.get(10, 1008), Some(0));
        assert_eq!(fb.get(100, 3), Some(0x00ff_ffff));
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
        let mut parser = SimdParser::new(fb);
        parser.parse(&input, &mut response);
    }
}
