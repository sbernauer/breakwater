use clap::Parser;
use const_format::formatcp;

pub const DEFAULT_NETWORK_BUFFER_SIZE: usize = 256 * 1024;
pub const DEFAULT_NETWORK_BUFFER_SIZE_STR: &str = formatcp!("{}", DEFAULT_NETWORK_BUFFER_SIZE);

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Listen address to bind to.
    /// The default value will listen on all interfaces for IPv4 and IPv6 packets.
    #[clap(short, long, default_value = "[::]:1234")]
    pub listen_address: String,

    /// Width of the drawing surface.
    #[clap(long, default_value_t = 1280)]
    pub width: usize,

    /// Height of the drawing surface.
    #[clap(long, default_value_t = 720)]
    pub height: usize,

    /// Frames per second the server should aim for.
    #[clap(short, long, default_value_t = 30)]
    pub fps: u32,

    /// The size in bytes of the network buffer used for each open TCP connection.
    /// Please use at least 64 KB (64_000 bytes).
    #[clap(long, default_value = DEFAULT_NETWORK_BUFFER_SIZE_STR, value_parser = 64_000..100_000_000)]
    pub network_buffer_size: i64,

    /// Text to display on the screen.
    #[clap(short, long, default_value = "Pixelflut server (breakwater)")]
    pub text: String,

    /// The font used to render the text on the screen.
    /// Should be a ttf file.
    /// If you use the default value a copy that ships with breakwater will be used - no need to download and provide the font.
    #[clap(long, default_value = "Arial.ttf")]
    pub font: String,

    /// Listen address the prometheus exporter should listen on.
    #[clap(short, long, default_value = "[::]:9100")]
    pub prometheus_listen_address: String,

    /// Save file where statistics are periodically saved.
    /// The save file will be read during startup and statistics are restored.
    /// To reset the statistics simply remove the file.
    #[clap(long, default_value = "statistics.json")]
    pub statistics_save_file: String,

    /// Interval (in seconds) in which the statistics save file should be updated.
    #[clap(long, default_value = "10")]
    pub statistics_save_interval_s: u64,

    /// Disable periodical saving of statistics into save file.
    #[clap(long)]
    pub disable_statistics_save_file: bool,

    /// Enable rtmp streaming to configured address, e.g. `rtmp://127.0.0.1:1935/live/test`
    #[clap(long)]
    pub rtmp_address: Option<String>,

    /// Enable dump of video stream into file. File location will be `<VIDEO_SAVE_FOLDER>/pixelflut_dump_{timestamp}.mp4
    #[clap(long)]
    pub video_save_folder: Option<String>,

    /// Allow only a certain number of connections per ip address
    #[clap(short, long)]
    pub connections_per_ip: Option<u64>,

    /// Enabled a VNC server
    #[clap(long)]
    pub vnc: bool,

    /// Port of the VNC server.
    #[cfg(feature = "vnc")]
    #[clap(short, long, default_value_t = 5900)]
    pub vnc_port: u16,

    /// Enable native display output. This requires some form of graphical system (so will probably not work on your
    /// server).
    #[clap(long)]
    pub native_display: bool,
}
