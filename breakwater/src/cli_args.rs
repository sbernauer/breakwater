use std::net::SocketAddr;

use crate::sinks::cli_args::SinkCliArgs;

pub const DEFAULT_NETWORK_BUFFER_SIZE: usize = 256 * 1024;

#[derive(clap::Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Width of the drawing surface.
    #[clap(long, default_value_t = 1280)]
    pub width: usize,

    /// Height of the drawing surface.
    #[clap(long, default_value_t = 720)]
    pub height: usize,

    /// Frames per second the server should aim for.
    #[clap(short, long, default_value_t = 30)]
    pub fps: u32,

    /// Listen address the Prometheus exporter should listen on.
    #[clap(short, long, default_value = "[::]:9100")]
    pub prometheus_listen_address: String,

    /// Create (or use an existing) shared memory region for the framebuffer.
    /// This enables other applications to read and write Pixel values to the framebuffer or can be
    /// used to persist the canvas across restarts.
    #[clap(long)]
    pub shared_memory_name: Option<String>,

    #[clap(flatten)]
    pub network_listener: NetworkListenerCliArgs,

    #[clap(flatten)]
    pub statistics_save_file: StatisticsSaveFileCliArgs,

    #[clap(flatten)]
    pub sinks: SinkCliArgs,
}

#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Network listener options")]
pub struct NetworkListenerCliArgs {
    /// Listen address to bind to (multiple can be specified).
    /// The default value will listen on all interfaces for IPv4 and IPv6 packets.
    #[clap(short, long = "listener-address", default_value = "[::]:1234")]
    pub listen_addresses: Vec<SocketAddr>,

    /// Specify one or more pixelflut endpoints to display to spectators.
    ///
    /// By default they will be derived from the `--listener-address`es specified. Use this to
    /// advertise custom addresses to connect to.
    #[clap(long = "advertised-endpoint")]
    pub advertised_endpoints: Vec<SocketAddr>,

    /// The size in bytes of the network buffer used for each open TCP connection.
    /// Use at least 64 KB (64_000 bytes).
    #[clap(
        long,
        default_value_t = DEFAULT_NETWORK_BUFFER_SIZE,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(64_000..=100_000_000),
    )]
    pub network_buffer_size: usize,

    /// Allow only a certain number of connections per ip address
    #[clap(short, long)]
    pub connections_per_ip: Option<u64>,
}

impl NetworkListenerCliArgs {
    /// Resolves the Pixelflut endpoints to advertise to users (so they know where to connect).
    ///
    /// If `--advertised-endpoint`s is set, those are returned verbatim. Otherwise we make a best
    /// effort guess: For a single listener we resolve the local v4 + v6 IPs and append the port,
    /// for multiple listeners we just list them.
    pub fn resolve_advertised_endpoints(&self) -> Vec<SocketAddr> {
        if !self.advertised_endpoints.is_empty() {
            return self.advertised_endpoints.clone();
        }

        match &self.listen_addresses[..] {
            // No listeners given, so also no endpoints to advertise
            [] => vec![],
            // In case of a single listener we get the local IPs (v4 + v6) and concat them with the
            // port
            [single_listener] => {
                let port = single_listener.port();

                [local_ip_address::local_ip(), local_ip_address::local_ipv6()]
                    .into_iter()
                    .filter_map(Result::ok)
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect()
            }
            // If multiple listeners are used it's complicated, so we just print them
            multiple_listeners => multiple_listeners.to_vec(),
        }
    }
}

#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Statistics save file options")]
pub struct StatisticsSaveFileCliArgs {
    /// Disable periodical saving of statistics into save file.
    #[clap(long)]
    pub disable_statistics_save_file: bool,

    /// Save file where statistics are periodically saved.
    /// The save file will be read during startup and statistics are restored.
    /// To reset the statistics simply remove the file.
    #[clap(long, default_value = "statistics.json")]
    pub statistics_save_file: String,

    /// Interval in which the statistics save file should be updated.
    ///
    /// Supports human durations such `10s` or `5m`.
    #[clap(long, default_value = "10s")]
    pub statistics_save_interval: humantime::Duration,
}
