use crate::framebuffer::FrameBuffer;
use const_format::formatcp;
use log::{info, warn};
use std::arch::x86_64::{
    __m256i, __m512i, _mm256_cmpeq_epi8, _mm256_extract_epi8, _mm256_loadu_epi8,
    _mm256_movemask_epi8, _mm256_set1_epi8, _mm512_castsi256_si512, _mm512_castsi512_si256,
    _mm512_loadu_epi8, _mm512_loadu_si512, _mm512_set1_epi8, _mm512_set_epi8, _mm512_shuffle_epi8,
    _mm512_sub_epi8,
};
use std::simd::{u32x8, u8x32, u8x64, Simd, SimdUint};
use std::sync::{Arc, OnceLock};
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
    // SHUFFLE_PATTERNS.get_or_init(|| unsafe { manually_calculate_shuffle_patterns() });

    let mut last_byte_parsed = 0;
    let mut connection_x_offset = parser_state.connection_x_offset;
    let mut connection_y_offset = parser_state.connection_y_offset;

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

            i += bytes_parsed as usize;
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

            let (x, y, present) = parse_pixel_coordinates(buffer.as_ptr(), &mut i);

            // End of command to set offset
            if present && unsafe { *buffer.get_unchecked(i) } == b'\n' {
                last_byte_parsed = i;
                connection_x_offset = x;
                connection_y_offset = y;
                continue;
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

struct ParseResult {
    bytes_parsed: u8,
    x: u16,
    y: u16,
    // aabbggrr
    rgba: u32,
    read_request: bool,
    has_alpha: bool,
}

// Longest possible space bitmask = "1234 1234 " => 10 chars
const SPACES_BITMASK_MASK: u32 = 0b0000_0000_0000_0000_0011_1111_1111;

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

    unsafe {
        // TODO: Get this into constants, but `_mm512_set1_epi8` is not a const function :/
        let ascii_zeros: __m512i = _mm512_set1_epi8(b'0' as i8);
        let ascii_spaces: __m256i = _mm256_set1_epi8(b' ' as i8);

        // TODO: We get u8 and pass i8 in here. Check if this causes problems
        let chars = _mm256_loadu_epi8(bytes.as_ptr() as *const i8);
        let chars512 = _mm512_castsi256_si512(chars);
        let digits = _mm512_sub_epi8(chars512, ascii_zeros);
        let spaces = _mm256_cmpeq_epi8(chars, ascii_spaces);

        // ATTENTION: Bitmask starts with LSB (so kind of wrong order)
        // let spaces_bitmask = chars.simd_eq(SIMD_SPACE_CHAR).to_bitmask();
        // let newline_bitmask = chars.simd_eq(SIMD_NEWLINE_CHAR).to_bitmask();
        // let spaces_bitmask = (spaces_bitmask | newline_bitmask) & SPACES_BITMASK_MASK;
        let spaces_bitmask = _mm256_movemask_epi8(spaces) as u32;
        let spaces_bitmask = spaces_bitmask & SPACES_BITMASK_MASK;

        // SAFETY: As SHUFFLE_PATTERNS has length `u16::MAX as usize + 1` and we use a us16 to index into it it will always succeed
        let (bytes_parsed, shuffle_pattern) =
            *SHUFFLE_PATTERNS.get_unchecked(spaces_bitmask as usize);

        let shuffle_pattern = _mm512_loadu_epi8(shuffle_pattern.as_ptr() as *const i8);
        let shuffled = _mm512_shuffle_epi8(digits, shuffle_pattern);
        let shuffled = _mm512_castsi512_si256(shuffled);

        let x = _mm256_extract_epi8(shuffled, 30);

        dbg!(String::from_utf8_unchecked(bytes.to_vec()));
        println!("{spaces_bitmask:#032b}");
        dbg!(digits);
        dbg!(shuffle_pattern);
        dbg!(shuffled);
        dbg!(x);

        ParseResult {
            bytes_parsed,
            x: 0,
            y: 0,
            rgba: 0,
            has_alpha: false,
            read_request: false,
        }
    }
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
fn parse_pixel_coordinates(buffer: *const u8, current_index: &mut usize) -> (usize, usize, bool) {
    let (x, x_visited) = parse_coordinate(buffer, current_index);
    *current_index += 1;
    let (y, y_visited) = parse_coordinate(buffer, current_index);
    (x, y, x_visited && y_visited)
}

const SHUFFLE_PATTERNS: [(u8, [u8; 64]); u16::MAX as usize + 1] =
    manually_calculate_shuffle_patterns();

// Let's add the stuff manually, we can always automate later
const fn manually_calculate_shuffle_patterns() -> [(u8, [u8; 64]); u16::MAX as usize + 1] {
    let mut shuffle_patterns = [(0, [255; 64]); u16::MAX as usize + 1];

    // 9 9
    shuffle_patterns[0b0000_0000_0000_1010] = (
        3,
        [
            255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
            255, 255, 255, 255, 255, 255, 2, 255, // y coordinate
            255, 255, 255, 255, 255, 255, 255, 255, // red + green
            255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
            255, 255, 255, 255, 255, 255, 255, 255, // padding
        ],
    );

    // // 9 99
    // shuffle_patterns[0b0000_0000_0001_0010] = (
    //     4,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
    //         255, 255, 255, 255, 2, 255, 3, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9 999
    // shuffle_patterns[0b0000_0000_0010_0010] = (
    //     5,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
    //         255, 255, 2, 255, 3, 255, 4, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9 9999
    // shuffle_patterns[0b0000_0000_0100_0010] = (
    //     6,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
    //         2, 255, 3, 255, 4, 255, 5, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 99 9
    // shuffle_patterns[0b0000_0000_0001_0100] = (
    //     4,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
    //         255, 255, 255, 255, 255, 255, 3, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 99 99
    // shuffle_patterns[0b0000_0000_0010_0100] = (
    //     5,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
    //         255, 255, 255, 255, 3, 255, 4, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 99 999
    // shuffle_patterns[0b0000_0000_0100_0100] = (
    //     6,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
    //         255, 255, 3, 255, 4, 255, 5, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 99 9999
    // shuffle_patterns[0b0000_0000_1000_0100] = (
    //     7,
    //     u8x32::from_array([
    //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
    //         3, 255, 4, 255, 5, 255, 6, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 999 9
    // shuffle_patterns[0b0000_0000_0010_1000] = (
    //     5,
    //     u8x32::from_array([
    //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
    //         255, 255, 255, 255, 255, 255, 4, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 999 99
    // shuffle_patterns[0b0000_0000_0100_1000] = (
    //     6,
    //     u8x32::from_array([
    //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
    //         255, 255, 255, 255, 4, 255, 5, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 999 999
    // shuffle_patterns[0b0000_0000_1000_1000] = (
    //     7,
    //     u8x32::from_array([
    //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
    //         255, 255, 4, 255, 5, 255, 6, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 999 9999
    // shuffle_patterns[0b0000_0001_0000_1000] = (
    //     8,
    //     u8x32::from_array([
    //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
    //         4, 255, 5, 255, 6, 255, 7, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9999 9
    // shuffle_patterns[0b0000_0000_0101_0000] = (
    //     6,
    //     u8x32::from_array([
    //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
    //         255, 255, 255, 255, 255, 255, 5, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9999 99
    // shuffle_patterns[0b0000_0000_1001_0000] = (
    //     7,
    //     u8x32::from_array([
    //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
    //         255, 255, 255, 255, 5, 255, 6, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9999 999
    // shuffle_patterns[0b0000_0001_0001_0000] = (
    //     8,
    //     u8x32::from_array([
    //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
    //         255, 255, 5, 255, 6, 255, 7, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    // // 9999 9999
    // shuffle_patterns[0b0000_0010_0001_0000] = (
    //     9,
    //     u8x32::from_array([
    //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
    //         5, 255, 6, 255, 7, 255, 8, 255, // y coordinate
    //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
    //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
    //     ]),
    // );

    shuffle_patterns
}

// static SHUFFLE_PATTERNS: OnceLock<[(u8, __m512i); u16::MAX as usize + 1]> = OnceLock::new();

// // Let's add the stuff manually, we can always automate later
// unsafe fn manually_calculate_shuffle_patterns() -> [(u8, __m512i); u16::MAX as usize + 1] {
//     let mut shuffle_patterns = [(0, _mm512_set1_epi8(i8::MAX)); u16::MAX as usize + 1];

//     // 9 9
//     shuffle_patterns[0b0000_0000_0000_1010] = (
//         3,
//         _mm512_set1_epi8(i8::MAX), // u8x32::from_array([
//                                    //     255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
//                                    //     255, 255, 255, 255, 255, 255, 2, 255, // y coordinate
//                                    //     255, 255, 255, 255, 255, 255, 255, 255, // red + green
//                                    //     255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//                                    // ]),
//     );

//     // // 9 99
//     // shuffle_patterns[0b0000_0000_0001_0010] = (
//     //     4,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
//     //         255, 255, 255, 255, 2, 255, 3, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9 999
//     // shuffle_patterns[0b0000_0000_0010_0010] = (
//     //     5,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
//     //         255, 255, 2, 255, 3, 255, 4, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9 9999
//     // shuffle_patterns[0b0000_0000_0100_0010] = (
//     //     6,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 255, 255, 0, 255, // X coordinate
//     //         2, 255, 3, 255, 4, 255, 5, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 99 9
//     // shuffle_patterns[0b0000_0000_0001_0100] = (
//     //     4,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
//     //         255, 255, 255, 255, 255, 255, 3, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 99 99
//     // shuffle_patterns[0b0000_0000_0010_0100] = (
//     //     5,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
//     //         255, 255, 255, 255, 3, 255, 4, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 99 999
//     // shuffle_patterns[0b0000_0000_0100_0100] = (
//     //     6,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
//     //         255, 255, 3, 255, 4, 255, 5, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 99 9999
//     // shuffle_patterns[0b0000_0000_1000_0100] = (
//     //     7,
//     //     u8x32::from_array([
//     //         255, 255, 255, 255, 0, 255, 1, 255, // X coordinate
//     //         3, 255, 4, 255, 5, 255, 6, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 999 9
//     // shuffle_patterns[0b0000_0000_0010_1000] = (
//     //     5,
//     //     u8x32::from_array([
//     //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
//     //         255, 255, 255, 255, 255, 255, 4, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 999 99
//     // shuffle_patterns[0b0000_0000_0100_1000] = (
//     //     6,
//     //     u8x32::from_array([
//     //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
//     //         255, 255, 255, 255, 4, 255, 5, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 999 999
//     // shuffle_patterns[0b0000_0000_1000_1000] = (
//     //     7,
//     //     u8x32::from_array([
//     //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
//     //         255, 255, 4, 255, 5, 255, 6, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 999 9999
//     // shuffle_patterns[0b0000_0001_0000_1000] = (
//     //     8,
//     //     u8x32::from_array([
//     //         255, 255, 0, 255, 1, 255, 2, 255, // X coordinate
//     //         4, 255, 5, 255, 6, 255, 7, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9999 9
//     // shuffle_patterns[0b0000_0000_0101_0000] = (
//     //     6,
//     //     u8x32::from_array([
//     //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
//     //         255, 255, 255, 255, 255, 255, 5, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9999 99
//     // shuffle_patterns[0b0000_0000_1001_0000] = (
//     //     7,
//     //     u8x32::from_array([
//     //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
//     //         255, 255, 255, 255, 5, 255, 6, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9999 999
//     // shuffle_patterns[0b0000_0001_0001_0000] = (
//     //     8,
//     //     u8x32::from_array([
//     //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
//     //         255, 255, 5, 255, 6, 255, 7, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     // // 9999 9999
//     // shuffle_patterns[0b0000_0010_0001_0000] = (
//     //     9,
//     //     u8x32::from_array([
//     //         0, 255, 1, 255, 2, 255, 3, 255, // X coordinate
//     //         5, 255, 6, 255, 7, 255, 8, 255, // y coordinate
//     //         255, 255, 255, 255, 255, 255, 255, 255, // red + green
//     //         255, 255, 255, 255, 255, 255, 255, 255, // blue + padding
//     //     ]),
//     // );

//     shuffle_patterns
// }
