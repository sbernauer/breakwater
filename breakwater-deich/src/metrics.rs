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

use color_eyre::eyre::{self, Context};
use prometheus_exporter::prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, exponential_buckets, register_histogram,
    register_histogram_vec, register_int_counter,
};
use tracing::info;
use uuid::Uuid;

/// How often each role logs a metrics summary.
pub const METRICS_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Buckets for frame-timing histograms: 0.5 ms … ~1 s, doubling. Covers the expected ~1–50 ms send
/// times with headroom to catch pathological tails.
fn frame_timing_buckets() -> Vec<f64> {
    exponential_buckets(0.0005, 2.0, 12).expect("valid bucket parameters")
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
