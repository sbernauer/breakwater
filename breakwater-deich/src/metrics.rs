//! Per-frame performance metrics shared by the worker and collector.
//!
//! Everything here is measured once per *frame* (≈ fps × workers events/s), never per pixel, so it
//! stays entirely off the Pixelflut hot path — a couple of clock reads and a histogram observe per
//! frame. Metrics register into the global Prometheus registry; [`serve`] exposes it for scraping,
//! and each role also logs a periodic summary (see the `log_metrics` loops in `worker`/`collector`),
//! so the numbers are available with or without Prometheus.
//!
//! The worker and collector measure genuinely different things, so they keep separate metric sets
//! ([`WorkerMetrics`] vs [`CollectorMetrics`]); the plumbing they share — the exporter and the timing
//! buckets — lives here.

use std::{net::SocketAddr, time::Duration};

use breakwater::statistics::StatisticsInformationEvent;
use color_eyre::eyre::{self, Context};
use prometheus_exporter::prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntGauge, IntGaugeVec, register_histogram,
    register_histogram_vec, register_int_counter, register_int_gauge, register_int_gauge_vec,
};
use tokio::sync::broadcast;
use tracing::info;
use uuid::Uuid;

/// How often each role logs a metrics summary.
pub const METRICS_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Buckets for frame-timing histograms (seconds). A coarse spread covers the fast path (sub-ms
/// frames) and the pathological tail, but the bulk is a fine 20 ms grid across 60–300 ms — the range
/// where real send/receive/latency times actually cluster.
///
/// Without that grid nearly every sample lands in a single 64–128 ms bucket, so `histogram_quantile`
/// interpolates across its whole 64 ms width: the quantile curve sits pinned near the bucket edge
/// and then jumps in coarse steps instead of tracking the real latency. The 20 ms grid resolves it.
fn frame_timing_buckets() -> Vec<f64> {
    let mut buckets = vec![0.001, 0.005, 0.01, 0.025, 0.05];
    // Fine 20 ms grid, 60 ms ..= 300 ms.
    let mut ms: u32 = 60;
    while ms <= 300 {
        buckets.push(f64::from(ms) / 1000.0);
        ms += 20;
    }
    buckets.extend([0.4, 0.5, 0.75, 1.0, 2.0]);
    buckets
}

/// Histogram options for a per-frame duration metric (in seconds), with the shared timing buckets.
fn frame_timing_opts(name: &str, help: &str) -> HistogramOpts {
    HistogramOpts::new(name, help).buckets(frame_timing_buckets())
}

/// Starts the Prometheus HTTP endpoint serving the global registry. Shared by both roles.
pub fn serve(listen_address: SocketAddr) -> eyre::Result<()> {
    prometheus_exporter::start(listen_address)
        .with_context(|| format!("failed to start Prometheus exporter on {listen_address}"))?;
    info!(%listen_address, "Serving Prometheus metrics");
    Ok(())
}

/// Worker-side metrics: how long each framebuffer push takes and how many frames we fell behind on.
pub struct WorkerMetrics {
    frame_send: Histogram,
    frames_lagged: IntCounter,
}

impl WorkerMetrics {
    pub fn new() -> eyre::Result<Self> {
        Ok(Self {
            frame_send: register_histogram!(frame_timing_opts(
                "deich_worker_frame_send_seconds",
                "Wall-clock time to push one framebuffer to the collector (marker + blob + flush)",
            ))?,
            frames_lagged: register_int_counter!(
                "deich_worker_frames_lagged_total",
                "Frames skipped versus the target FPS because a prior send overran its slot",
            )?,
        })
    }

    pub fn observe_send(&self, duration: Duration) {
        self.frame_send.observe(duration.as_secs_f64());
    }

    pub fn add_lagged(&self, frames: u64) {
        self.frames_lagged.inc_by(frames);
    }

    /// Cumulative (send count, total send seconds); the log loop diffs these into a per-interval mean.
    pub fn send_totals(&self) -> (u64, f64) {
        (
            self.frame_send.get_sample_count(),
            self.frame_send.get_sample_sum(),
        )
    }

    pub fn lagged_total(&self) -> u64 {
        self.frames_lagged.get()
    }
}

/// Collector-side metrics: per-worker receive time and end-to-end latency, plus total ingress bytes.
pub struct CollectorMetrics {
    frame_recv: HistogramVec,
    frame_latency: HistogramVec,
    ingress_bytes: IntCounter,
}

impl CollectorMetrics {
    pub fn new() -> eyre::Result<Self> {
        Ok(Self {
            frame_recv: register_histogram_vec!(
                frame_timing_opts(
                    "deich_collector_frame_recv_seconds",
                    "Time to read one framebuffer body off the wire, per worker",
                ),
                &["worker"],
            )?,
            frame_latency: register_histogram_vec!(
                frame_timing_opts(
                    "deich_collector_frame_latency_seconds",
                    "Worker send-start to collector recv-done latency, per worker (needs clock sync)",
                ),
                &["worker"],
            )?,
            ingress_bytes: register_int_counter!(
                "deich_collector_ingress_bytes_total",
                "Total framebuffer bytes received from all workers",
            )?,
        })
    }

    /// Returns a recorder with this worker's per-label histogram handles cached, so the per-frame hot
    /// path avoids a label-map lookup.
    pub fn recorder(&self, worker_id: Uuid) -> FrameRecorder {
        let label = worker_id.to_string();
        FrameRecorder {
            recv: self.frame_recv.with_label_values(&[&label]),
            latency: self.frame_latency.with_label_values(&[&label]),
            ingress_bytes: self.ingress_bytes.clone(),
        }
    }

    pub fn ingress_bytes_total(&self) -> u64 {
        self.ingress_bytes.get()
    }
}

/// Per-connection recorder holding one worker's cached metric handles.
pub struct FrameRecorder {
    recv: Histogram,
    latency: Histogram,
    ingress_bytes: IntCounter,
}

impl FrameRecorder {
    pub fn observe_frame(&self, recv: Duration, latency: Duration, bytes: u64) {
        self.recv.observe(recv.as_secs_f64());
        self.latency.observe(latency.as_secs_f64());
        self.ingress_bytes.inc_by(bytes);
    }
}

/// Collector-side per-IP client statistics — bytes, connections and denied connections per IP, plus
/// the connected-address counts. Exposed with the **same metric names and labels as breakwater**
/// (`breakwater_bytes`, `breakwater_connections`, …) so existing breakwater dashboards work against
/// the collector unchanged.
///
/// Fed by the aggregated [`StatisticsInformationEvent`] the collector already publishes: `bytes` and
/// `denied_connections` are the persistent grand totals (monotonic, survive restarts), while
/// `connections` is the live gauge. (breakwater's VNC-only `breakwater_frame` has no meaning here —
/// workers don't render — so it is deliberately omitted.)
pub struct StatisticsMetrics {
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,

    ips_v6: IntGauge,
    ips_v4: IntGauge,
    statistic_events: IntGauge,

    connections_for_ip: IntGaugeVec,
    denied_connections_for_ip: IntGaugeVec,
    bytes_for_ip: IntGaugeVec,
}

impl StatisticsMetrics {
    pub fn new(
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    ) -> eyre::Result<Self> {
        Ok(Self {
            statistics_information_rx,
            ips_v6: register_int_gauge!(
                "breakwater_ips_v6",
                "Total number of connected IPv6 addresses"
            )?,
            ips_v4: register_int_gauge!(
                "breakwater_ips_v4",
                "Total number of connected IPv4 addresses"
            )?,
            statistic_events: register_int_gauge!(
                "breakwater_statistic_events",
                "Number of statistics events send internally"
            )?,
            connections_for_ip: register_int_gauge_vec!(
                "breakwater_connections",
                "Number of client connections per IP address",
                &["ip"],
            )?,
            denied_connections_for_ip: register_int_gauge_vec!(
                "breakwater_denied_connections",
                "Number of denied connections per IP address because it tried to open too many connections",
                &["ip"],
            )?,
            bytes_for_ip: register_int_gauge_vec!(
                "breakwater_bytes",
                "Number of bytes received per IP address",
                &["ip"],
            )?,
        })
    }

    /// Updates the gauges from each published statistics event until the channel closes. A lag just
    /// means intermediate snapshots were dropped — the next event is fully authoritative, so we skip
    /// the gap and carry on.
    pub async fn run(&mut self) {
        loop {
            match self.statistics_information_rx.recv().await {
                Ok(event) => self.update(&event),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    fn update(&self, event: &StatisticsInformationEvent) {
        self.ips_v6.set(i64::from(event.ips_v6));
        self.ips_v4.set(i64::from(event.ips_v4));
        self.statistic_events.set(event.statistic_events as i64);

        // Reset the per-IP vectors before repopulating: an IP that dropped out of the event (e.g.
        // all its connections closed) would otherwise linger at its last value forever. This mirrors
        // breakwater's exporter.
        self.connections_for_ip.reset();
        for (ip, connections) in &event.connections_for_ip {
            self.connections_for_ip
                .with_label_values(&[&ip.to_string()])
                .set(i64::from(*connections));
        }
        self.denied_connections_for_ip.reset();
        for (ip, denied) in &event.denied_connections_for_ip {
            self.denied_connections_for_ip
                .with_label_values(&[&ip.to_string()])
                .set(i64::from(*denied));
        }
        self.bytes_for_ip.reset();
        for (ip, bytes) in &event.bytes_for_ip {
            self.bytes_for_ip
                .with_label_values(&[&ip.to_string()])
                .set(*bytes as i64);
        }
    }
}
