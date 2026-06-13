use std::{net::SocketAddr, path::PathBuf};

use breakwater::cli_args::DEFAULT_NETWORK_BUFFER_SIZE_STR;
use clap::{Args, Parser, Subcommand};

/// `deich` is breakwater's distributed mode: many workers each run a Pixelflut server and sync
/// their framebuffer to a single collector.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub role: Role,
}

#[derive(Subcommand, Debug)]
pub enum Role {
    /// Run a worker: a Pixelflut server that continuously syncs its framebuffer to the collector.
    /// Canvas geometry and frame rate are dictated by the collector.
    Worker(WorkerArgs),

    /// Run the collector: gathers and merges the framebuffers of all workers.
    Collector(CollectorArgs),
}

#[derive(Args, Debug)]
pub struct WorkerArgs {
    /// Listen address to bind the Pixelflut server to (multiple can be specified).
    /// The default value will listen on all interfaces for IPv4 and IPv6 packets.
    #[clap(short, long = "listen-address", default_value = "[::]:1234")]
    pub listen_addresses: Vec<SocketAddr>,

    /// The size in bytes of the network buffer used for each open TCP connection.
    /// Please use at least 64 KB (64_000 bytes).
    #[clap(
        long,
        default_value = DEFAULT_NETWORK_BUFFER_SIZE_STR,
        value_parser = 64_000..100_000_000,
    )]
    pub network_buffer_size: i64,

    /// Address of the collector to fetch the config from and stream the framebuffer to.
    #[clap(long, default_value = "127.0.0.1:9999")]
    pub collector_address: String,

    /// File storing this worker's persistent UUID, created on first run. The UUID identifies the
    /// worker to the collector across restarts, which keeps per-worker stats stable over time.
    #[clap(long, default_value = "worker-id")]
    pub worker_id_file: PathBuf,
}

#[derive(Args, Debug)]
pub struct CollectorArgs {
    /// Listen address the workers connect to.
    #[clap(short, long, default_value = "[::]:9999")]
    pub listen_address: SocketAddr,

    /// Width of the canvas. Sent to every worker as part of its config.
    #[clap(long, default_value_t = 1920)]
    pub width: u32,

    /// Height of the canvas. Sent to every worker as part of its config.
    #[clap(long, default_value_t = 1080)]
    pub height: u32,

    /// Frames per second the workers should sync at. Sent to every worker as part of its config.
    /// Must stay well below the ~536 ms window the per-pixel timestamp can represent.
    #[clap(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=60))]
    pub fps: u32,
}
