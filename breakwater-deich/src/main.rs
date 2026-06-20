use breakwater_deich::cli_args::{CliArgs, Role};
use clap::Parser;
use color_eyre::eyre;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = CliArgs::parse();
    breakwater_deich::init_telemetry()?;

    match cli.role {
        Role::Worker(args) => breakwater_deich::worker::run(args).await,
        Role::Collector(args) => breakwater_deich::collector::run(args).await,
    }
}
