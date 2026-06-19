use std::net::SocketAddr;

use const_format::formatcp;

use crate::sinks::cli_args::SinkCliArgs;

pub const DEFAULT_NETWORK_BUFFER_SIZE: usize = 256 * 1024;
pub const DEFAULT_NETWORK_BUFFER_SIZE_STR: &str = formatcp!("{}", DEFAULT_NETWORK_BUFFER_SIZE);

#[derive(clap::Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Listen address to bind to (multiple can be specified).
    /// The default value will listen on all interfaces for IPv4 and IPv6 packets.
    #[clap(short, long = "listener-address", default_value = "[::]:1234")]
    pub listen_addresses: Vec<SocketAddr>,

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
    #[clap(
        long,
        default_value = DEFAULT_NETWORK_BUFFER_SIZE_STR,
        value_parser = 64_000..100_000_000,
    )]
    pub network_buffer_size: i64,

    /// Listen address the Prometheus exporter should listen on.
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

    /// Allow only a certain number of connections per ip address
    #[clap(short, long)]
    pub connections_per_ip: Option<u64>,

    /// Create (or use an existing) shared memory region for the framebuffer.
    /// This enables other applications to read and write Pixel values to the framebuffer or can be
    /// used to persist the canvas across restarts.
    #[clap(long)]
    pub shared_memory_name: Option<String>,

    #[clap(flatten)]
    pub sinks: SinkCliArgs,
}
