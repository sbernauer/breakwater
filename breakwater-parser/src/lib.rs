// Needed for simple implementation
#![feature(portable_simd)]

use const_format::formatcp;

#[cfg(target_arch = "x86_64")]
mod assembler;
mod framebuffer;
mod memchr;
mod original;
mod refactored;

#[cfg(target_arch = "x86_64")]
pub use assembler::AssemblerParser;
pub use framebuffer::{
    FB_BYTES_PER_PIXEL, FrameBuffer, shared_memory::SharedMemoryFrameBuffer,
    simple::SimpleFrameBuffer,
};
pub use memchr::MemchrParser;
pub use original::OriginalParser;
pub use refactored::RefactoredParser;

pub const HELP_TEXT: &[u8] = formatcp!("\
Pixelflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color rrggbb
{}
PX x y gg: Color the pixel (x,y) with the hexadecimal color gggggg. Basically this is the same as the other commands, but is a more efficient way of filling white, black or gray areas
PX x y: Get the color value of the pixel (x,y)
{}{}SIZE: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
OFFSET x y: Apply offset (x,y) to all further pixel draws on this connection. This can e.g. be used to pre-calculate an image/animation and simply use the OFFSET command to move it around the screen without the need to re-calculate it
",
if cfg!(feature = "alpha") {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb and a transparency of aa, where ff means draw normally on top of the existing pixel and 00 means fully transparent (no change at all)"
} else {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb. The alpha part is discarded for performance reasons, as breakwater was compiled without the alpha feature"
},
if cfg!(feature = "binary-set-pixel") {
    "PBxxyyrgba: Binary version of the PX command. x and y are little-endian 16 bit coordinates, r, g, b and a are a byte each. There is *no* newline after the command.\n"
} else {
    ""
},
if cfg!(feature = "binary-sync-pixels") {
    "PXMULTI<startX:16><startY:16><len:32><rgba 1 of (startX, startY)><rgba 2 of (startX + 1, startY)><rgba 3 of (startX + 1, startY)>...<rgba len>: EXPERIMENTAL binary syncing of whole pixel areas. Please note that for performance reasons this will be copied 1:1 to the servers framebuffer. The server will just take the following <len> bytes and memcpy it into the framebuffer, so the alpha channel doesn't matter and you might mess up the screen. This is intended for export-use, especially when syncing or combining multiple Pixelflut screens across multiple servers\n"
} else {
    ""
},
).as_bytes();

pub const ALT_HELP_TEXT: &[u8] = b"Stop spamming HELP!\n";

pub trait Parser {
    /// Returns the last byte parsed. The next parsing loop will again contain all data that was not parsed.
    fn parse(&mut self, buffer: &[u8], response: &mut Vec<u8>) -> usize;

    // Sadly this cant be const (yet?) (https://github.com/rust-lang/rust/issues/71971 and https://github.com/rust-lang/rfcs/pull/2632)
    fn parser_lookahead(&self) -> usize;
}
