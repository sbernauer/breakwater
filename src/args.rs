use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Listen address to bind to.
    /// The default value will listen on all interfaces for IPv4 and v6 packets.
    #[clap(short, long, default_value = "[::]:1234")]
    pub listen_address: String,

    /// Port of the VNC server.
    #[clap(short, long, default_value_t = 5900)]
    pub vnc_port: u32,

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
    #[clap(short, long, default_value = "Pixelflut server (breakwater)")]
    pub text: String,

    /// The font used to render the text on the screen.
    /// Should be a ttf file.
    /// If you use the default value a copy that ships with breakwater will be used - no need to download and provide the font.
    #[clap(long, default_value = "Arial.ttf")]
    pub font: String,

    /// Listen address zhe prometheus exporter should listen om.
    /// The default value will listen on all interfaces for IPv4 and v6 packets.
    #[clap(short, long, default_value = "[::]:9100")]
    pub prometheus_listen_address: String,

    /// Save file where statistics are periodically saved.
    /// The save file will be read during startup and statistics are restored.
    /// To reset the statistics simply remove the file.
    #[clap(long, default_value = "statistics.json")]
    pub statistics_save_file: String,

    /// Interval (in seconds) in which the save file should be updated.
    #[clap(long, default_value = "10")]
    pub statistics_save_interval_s: u64,

    /// Disable periodical saving of statistics into save file.
    #[clap(long)]
    pub disable_statistics_save_file: bool,
}
