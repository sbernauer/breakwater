use color_eyre::eyre::{self, Context};
use tokio::sync::broadcast;

pub mod cli_args;
pub mod prometheus_exporter;
pub mod server;
pub mod sinks;
pub mod statistics;

mod connection_buffer;

#[cfg(test)]
mod test_helpers;

#[cfg(test)]
mod tests;

pub async fn handle_ctrl_c(terminate_signal_tx: broadcast::Sender<()>) -> eyre::Result<()> {
    tokio::signal::ctrl_c()
        .await
        .context("failed to wait for ctrl + c")?;

    terminate_signal_tx
        .send(())
        .context("failed to signal termination")?;

    Ok(())
}
