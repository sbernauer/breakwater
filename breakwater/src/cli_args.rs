use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
};

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
    /// We recommend `<hostname or IP>:<port>`, e.g. `pixelflut.example.com:1234`, `1.2.3.4:1234`
    /// or `[2001:db8::1]:1234`. The value is only ever displayed, so anything goes.
    ///
    /// By default they will be derived from the `--listener-address`es specified. Use this to
    /// advertise custom addresses to connect to.
    #[clap(long = "advertised-endpoint", value_name = "HOST:PORT")]
    pub advertised_endpoints: Vec<String>,

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
    /// effort guess based on the `--listener-address`es, see [`Self::resolve_ips`].
    pub fn resolve_advertised_endpoints(&self) -> Vec<String> {
        if !self.advertised_endpoints.is_empty() {
            return self.advertised_endpoints.clone();
        }

        self.listen_addresses
            .iter()
            .flat_map(|socket_addr| {
                let port = socket_addr.port();

                Self::resolve_ips(socket_addr.ip())
                    .into_iter()
                    .map(move |ip| SocketAddr::new(ip, port))
            })
            // Deduplicate. We use a `BTreeSet`, as the endpoints are displayed to spectators and
            // a `HashSet` would shuffle them on every restart.
            .collect::<BTreeSet<SocketAddr>>()
            .into_iter()
            .map(|socket_addr| socket_addr.to_string())
            .collect()
    }

    /// Resolves the IP of a listener address to the IPs worth advertising.
    ///
    /// Unspecified addresses (`0.0.0.0` and `[::]`) are useless to spectators, so we replace them
    /// with the local IPs of this machine. `[::]` also accepts IPv4 traffic on dual-stack systems,
    /// so we advertise both the IPv6 and the IPv4 address in that case.
    ///
    /// Returns an empty [`Vec`] if no local IP could be determined, e.g. when no interface is up.
    fn resolve_ips(ip: IpAddr) -> Vec<IpAddr> {
        if !ip.is_unspecified() {
            return vec![ip];
        }

        match ip {
            IpAddr::V4(_) => local_ip_address::local_ip().ok().into_iter().collect(),
            IpAddr::V6(_) => [
                local_ip_address::local_ipv6().ok(),
                local_ip_address::local_ip().ok(),
            ]
            .into_iter()
            .flatten()
            .collect(),
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn args(listen_addresses: &[&str], advertised_endpoints: &[&str]) -> NetworkListenerCliArgs {
        NetworkListenerCliArgs {
            listen_addresses: listen_addresses
                .iter()
                .map(|a| a.parse().unwrap())
                .collect(),
            advertised_endpoints: advertised_endpoints
                .iter()
                .map(ToString::to_string)
                .collect(),
            network_buffer_size: DEFAULT_NETWORK_BUFFER_SIZE,
            connections_per_ip: None,
        }
    }

    #[test]
    fn test_advertised_endpoints_take_precedence() {
        let args = args(&["1.2.3.4:1234"], &["pixelflut.example.com:1234"]);

        assert_eq!(
            args.resolve_advertised_endpoints(),
            vec!["pixelflut.example.com:1234"]
        );
    }

    #[test]
    fn test_specified_listener_addresses_are_deduplicated_and_sorted() {
        let args = args(
            &[
                "[2001:db8::1]:1234",
                "1.2.3.4:1234",
                "1.2.3.4:1234",
                "1.2.3.4:4321",
            ],
            &[],
        );

        assert_eq!(
            args.resolve_advertised_endpoints(),
            vec!["1.2.3.4:1234", "1.2.3.4:4321", "[2001:db8::1]:1234"]
        );
    }

    #[rstest]
    #[case("1.2.3.4")]
    #[case("2001:db8::1")]
    fn test_specified_ips_are_kept_as_is(#[case] ip: IpAddr) {
        assert_eq!(NetworkListenerCliArgs::resolve_ips(ip), vec![ip]);
    }
}
