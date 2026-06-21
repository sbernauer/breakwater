use breakwater_deich::cli_args::{CliArgs, Role};
use clap::{CommandFactory, FromArgMatches};
use color_eyre::eyre;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // We parse via `ArgMatches` (instead of `CliArgs::parse()`) so that `SinkCliArgs::validate` can use
    // `value_source` to tell which sink options were actually passed on the command line.
    let mut cmd = CliArgs::command();
    let matches = cmd.get_matches_mut();
    let args = CliArgs::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    breakwater_deich::init_telemetry()?;

    match args.role {
        Role::Worker(args) => breakwater_deich::worker::run(args).await,
        Role::Collector(args) => {
            if let Err(e) = args.sinks.validate(&mut cmd, &matches) {
                e.exit();
            }
            breakwater_deich::collector::run(args).await
        }
    }
}
