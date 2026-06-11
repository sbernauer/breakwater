use std::{net::SocketAddr, sync::Arc, time::Duration};

use breakwater::{
    cli_args::DEFAULT_NETWORK_BUFFER_SIZE, handle_ctrl_c, server::Server,
    statistics::StatisticsEvent,
};
use breakwater_parser::{TimeTrackingFrameBuffer, get_current_ns_since_unix_epoch};
use color_eyre::eyre::{self, Context};
use tokio::sync::{broadcast, mpsc};
use tracing::debug;

/// How often we sync the framebuffer
const SYNC_FPS: u64 = 30;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(if cfg!(debug_assertions) {
            tracing::Level::DEBUG.into()
        } else {
            tracing::Level::INFO.into()
        })
        .from_env()?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let fb = Arc::new(TimeTrackingFrameBuffer::new(
        1920,
        1080,
        get_current_ns_since_unix_epoch(),
    ));
    let (statistics_tx, statistics_rx) = mpsc::channel::<StatisticsEvent>(100);
    let (terminate_signal_tx, _terminate_signal_rx) = broadcast::channel::<()>(1);

    let mut server = Server::new(
        &["[::]:1234".parse::<SocketAddr>().unwrap()],
        fb.clone(),
        statistics_tx.clone(),
        DEFAULT_NETWORK_BUFFER_SIZE,
        None,
    )
    .await
    .context("failed to start pixelflut server")?;

    let server_listener_thread = tokio::spawn(async move { server.start().await });
    let stats_drain_thread = tokio::spawn(async move { drain_stats(statistics_rx).await });
    let sync_thread = {
        let fb = fb.clone();
        tokio::spawn(async move { sync_framebuffer(fb).await })
    };

    handle_ctrl_c(terminate_signal_tx).await?;
    server_listener_thread.abort();
    stats_drain_thread.abort();
    sync_thread.abort();

    Ok(())
}

/// Periodically syncs the framebuffer and re-bases its per-pixel timestamps to the current time.
///
/// The re-basing has to happen faster than the ~536 ms window the 3-byte `ns_since_base` can
/// represent; 30 fps (~33 ms) leaves plenty of headroom.
async fn sync_framebuffer(fb: Arc<TimeTrackingFrameBuffer>) {
    let mut interval = tokio::time::interval(Duration::from_nanos(1_000_000_000 / SYNC_FPS));
    // Let's try to never skip a frame and catch up to make it easy for the sync collection server
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    loop {
        interval.tick().await;

        // TODO: Actually sync the framebuffer somewhere instead of just logging its size.
        debug!(bytes = fb.num_bytes(), "Syncing framebuffer");

        // Re-base *after* the sync, so the bytes we just synced are still relative to the old
        // base. New writes from here on are relative to "now".
        fb.set_base_ns_since_unix_epoch(get_current_ns_since_unix_epoch());
    }
}

/// Currently we don't care about stats, so let's just drain them
async fn drain_stats(mut statistics_rx: mpsc::Receiver<StatisticsEvent>) {
    loop {
        if statistics_rx.recv().await.is_none() {
            return;
        }
    }
}
