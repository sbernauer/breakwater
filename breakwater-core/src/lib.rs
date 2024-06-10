use const_format::formatcp;

pub mod framebuffer;
pub mod test_helpers;

pub const HELP_TEXT: &[u8] = formatcp!("\
Pixelflut server powered by breakwater https://github.com/sbernauer/breakwater
Available commands:
HELP: Show this help
PX x y rrggbb: Color the pixel (x,y) with the given hexadecimal color rrggbb
{}
PX x y gg: Color the pixel (x,y) with the hexadecimal color gggggg. Basically this is the same as the other commands, but is a more efficient way of filling white, black or gray areas
PX x y: Get the color value of the pixel (x,y)
{}SIZE: Get the size of the drawing surface, e.g. `SIZE 1920 1080`
OFFSET x y: Apply offset (x,y) to all further pixel draws on this connection. This can e.g. be used to pre-calculate an image/animation and simply use the OFFSET command to move it around the screen without the need to re-calculate it
",
if cfg!(feature = "alpha") {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb and a transparency of aa, where ff means draw normally on top of the existing pixel and 00 means fully transparent (no change at all)"
} else {
    "PX x y rrggbbaa: Color the pixel (x,y) with the given hexadecimal color rrggbb. The alpha part is discarded for performance reasons, as breakwater was compiled without the alpha feature"
},
if cfg!(feature = "binary-commands") {
    "PBxxyyrgba: Binary version of the PX command. x and y are little-endian 16 bit coordinates, r, g, b and are a byte each. There is *no* newline after the command.\n"
} else {
    ""
}
).as_bytes();

pub const ALT_HELP_TEXT: &[u8] = b"Stop spamming HELP!\n";
pub const CONNECTION_DENIED_TEXT: &[u8] = b"Connection denied as connection limit is reached";
