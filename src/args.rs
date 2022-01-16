use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Listen address to bind to.
    /// The default value will listen on all interfaces for IPv4 and v6 packets.
    #[clap(short, long, default_value = "[::]:1234")]
    pub listen_address: String,

    /// Width of the drawing surface
    #[clap(short, long, default_value_t = 1280)]
    pub width: usize,

    /// Height of the drawing surface
    #[clap(short, long, default_value_t = 720)]
    pub height: usize,

    /// Frames per second the VNC server should aim for
    #[clap(short, long, default_value_t = 30)]
    pub fps: u32,

    /// Text to display on the screen.
    /// The text will be followed by "on <listen_address>"
    #[clap(short, long, default_value = "Breakwater Pixelflut server")]
    pub text: String,
}
