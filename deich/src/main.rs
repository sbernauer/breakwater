use std::{net::SocketAddr, sync::Arc};

use breakwater::{
    cli_args::DEFAULT_NETWORK_BUFFER_SIZE, handle_ctrl_c, server::Server,
    statistics::StatisticsEvent,
};
use breakwater_parser::{TimeTrackingFrameBuffer, get_current_ns_since_unix_epoch};
use color_eyre::eyre::{self, Context};
use tokio::sync::{broadcast, mpsc};

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

    handle_ctrl_c(terminate_signal_tx).await?;
    server_listener_thread.abort();
    stats_drain_thread.abort();

    Ok(())
}

/// Currently we don't care about stats, so let's just drain them
async fn drain_stats(mut statistics_rx: mpsc::Receiver<StatisticsEvent>) {
    loop {
        if statistics_rx.recv().await.is_none() {
            return;
        }
    }
}
