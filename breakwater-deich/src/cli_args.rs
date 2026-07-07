use std::{net::SocketAddr, path::PathBuf};

use breakwater::{
    cli_args::{NetworkListenerCliArgs, StatisticsSaveFileCliArgs},
    sinks::cli_args::SinkCliArgs,
};

#[derive(clap::Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub role: Role,
}

#[derive(clap::Subcommand, Debug)]
pub enum Role {
    /// Run a worker: A Pixelflut server that continuously syncs its framebuffer to the collector.
    /// Canvas geometry and frame rate are dictated by the collector.
    Worker(Box<WorkerCliArgs>),

    /// Run the collector: Gathers and merges the framebuffers of all workers.
    Collector(Box<CollectorCliArgs>),
}

#[derive(clap::Args, Debug)]
pub struct WorkerCliArgs {
    #[clap(flatten)]
    pub network_listener: NetworkListenerCliArgs,

    /// Address of the collector to fetch the config from and stream the framebuffer to.
    #[clap(long, default_value = "[::1]:1235")]
    pub collector_address: SocketAddr,

    /// File storing this worker's persistent UUID, created on first run. The UUID identifies the
    /// worker to the collector across restarts, which keeps per-worker stats stable over time.
    #[clap(long, default_value = "worker-id")]
    pub worker_id_file: PathBuf,

    #[clap(flatten)]
    pub prometheus: PrometheusCliArgs,
}

#[derive(clap::Args, Debug)]
pub struct CollectorCliArgs {
    /// Listen address the workers connect to.
    #[clap(short, long, default_value = "[::]:1235")]
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

    #[clap(flatten)]
    pub prometheus: PrometheusCliArgs,

    #[clap(flatten)]
    pub statistics_save_file: StatisticsSaveFileCliArgs,

    #[clap(flatten)]
    pub sinks: SinkCliArgs,
}

/// Prometheus metrics options, shared by both roles. Enabled by default (like breakwater): each node
/// serves `/metrics` on this address. Metrics are also summarized to the log on an interval, so
/// they're available even without a scraper.
#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Prometheus options")]
pub struct PrometheusCliArgs {
    /// Address to serve Prometheus metrics on. Workers and the collector run on separate hosts, so
    /// the same default port is fine for all of them; override it if you colocate roles on one host.
    #[clap(long, default_value = "[::]:9100")]
    pub prometheus_listen_address: SocketAddr,
}
