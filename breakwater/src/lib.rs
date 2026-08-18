use color_eyre::eyre;

pub mod cli_args;
pub mod connection_buffer;
#[cfg(feature = "prometheus")]
pub mod prometheus_exporter;
pub mod server;
pub mod sinks;
pub mod statistics;

/// Shared process setup for the breakwater binaries: error reporting and logging.
pub fn init_telemetry() -> eyre::Result<()> {
    color_eyre::install()?;

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(if cfg!(debug_assertions) {
            tracing::Level::DEBUG.into()
        } else {
            tracing::Level::INFO.into()
        })
        .from_env()?;
    tracing_subscriber::fmt().with_env_filter(filter).init();

    Ok(())
}

#[cfg(test)]
pub mod test_helpers;
#[cfg(test)]
pub mod tests;
