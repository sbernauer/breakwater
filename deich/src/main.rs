use clap::Parser;
use color_eyre::eyre;
use deich::cli_args::{Cli, Role};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    deich::init_telemetry()?;

    match cli.role {
        Role::Worker(args) => deich::worker::run(args).await,
        Role::Collector(args) => deich::collector::run(args).await,
    }
}
