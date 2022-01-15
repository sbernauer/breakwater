use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Listen address to bind to.
    /// The default value will listen on all interfaces for IPv4 and v6 packets.
    #[clap(short, long, default_value = "[::]:1234")]
    pub listen_address: String,

    /// Size of the thread pool handling the network traffic.
    #[clap(short, long, default_value = "12")]
    pub thread_pool_size: u32,

    /// Width of the display
    #[clap(short, long, default_value_t = 1280)]
    pub width: usize,

    /// Height of the display
    #[clap(short, long, default_value_t = 720)]
    pub height: usize,

    /// Frames per second the VNC server should aim for
    #[clap(short, long, default_value_t = 60)]
    pub fps: u32,
}
