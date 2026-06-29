use clap::{ValueEnum, error::ErrorKind, parser::ValueSource};

use crate::sinks::Sink;

#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Sink options")]
pub struct SinkCliArgs {
    /// Enable an arbitrary number of sinks (argument can be repeated). The availability of sinks depends on the enabled features.
    #[clap(short = 's', long = "enable-sink")]
    pub enabled_sinks: Vec<Sink>,

    #[clap(flatten)]
    pub ffmpeg_sink: crate::sinks::ffmpeg::FfmpegSinkCliArgs,

    #[cfg(feature = "egui")]
    #[clap(flatten)]
    pub egui_sink: crate::sinks::egui::EguiSinkCliArgs,

    // winit doesn't have any CLI arguments
    #[cfg(feature = "ndi")]
    #[clap(flatten)]
    pub ndi_sink: crate::sinks::ndi::NdiSinkCliArgs,

    #[cfg(feature = "vnc")]
    #[clap(flatten)]
    pub vnc_sink: crate::sinks::vnc::VncSinkCliArgs,
}

impl SinkCliArgs {
    /// Reject sink-specific options (e.g. `--vnc-text`) that were passed on the command line without the
    /// corresponding sink being enabled via `--enable-sink`.
    ///
    /// clap can not express this natively: its relational checks (`requires`, `conflicts_with`, ...) operate on
    /// argument presence, not on a specific value being contained in a multi-valued enum argument like
    /// `enabled_sinks`. So we detect explicitly-passed options via [`clap::ArgMatches::value_source`] and validate manually.
    pub fn validate(
        &self,
        cmd: &mut clap::Command,
        matches: &clap::ArgMatches,
    ) -> Result<(), clap::Error> {
        // Every sink-specific option is named with its sink as prefix (e.g. `--vnc-text` belongs to the
        // `vnc` sink). We rely on that convention to detect options that were passed without their sink
        // being enabled, instead of hardcoding the full list of arguments here.
        let sink_prefixes: Vec<(String, String, Sink)> = Sink::value_variants()
            .iter()
            .map(|sink| {
                let name = sink
                    .to_possible_value()
                    .expect("every Sink variant has a name")
                    .get_name()
                    .to_owned();
                (format!("{name}-"), name, *sink)
            })
            .collect();

        // We can't call `cmd.error` (needs `&mut cmd`) while iterating `cmd.get_arguments()` (borrows `cmd`),
        // so we first collect the offending (flag, sink name) and build the error afterwards.
        let mut conflict = None;
        'args: for arg in cmd.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            if matches.value_source(arg.get_id().as_str()) != Some(ValueSource::CommandLine) {
                continue;
            }
            for (prefix, name, sink) in &sink_prefixes {
                if long.starts_with(prefix) && !self.enabled_sinks.contains(sink) {
                    conflict = Some((format!("--{long}"), name.clone()));
                    break 'args;
                }
            }
        }

        if let Some((flag, sink_name)) = conflict {
            return Err(cmd.error(
                ErrorKind::ArgumentConflict,
                format!(
                    "the argument '{flag}' can only be used when the '{sink_name}' sink is enabled \
                     (pass '--enable-sink {sink_name}')"
                ),
            ));
        }

        // Validate that every enabled sink got the arguments it requires. Each sink encapsulates its own
        // requirements in a `validate` method, which we only invoke when that sink is actually enabled.
        if self.enabled_sinks.contains(&Sink::Ffmpeg) {
            self.ffmpeg_sink
                .validate()
                .map_err(|msg| cmd.error(ErrorKind::MissingRequiredArgument, msg))?;
        }
        #[cfg(feature = "vnc")]
        if self.enabled_sinks.contains(&Sink::Vnc) {
            self.vnc_sink
                .validate()
                .map_err(|msg| cmd.error(ErrorKind::MissingRequiredArgument, msg))?;
        }

        #[cfg(all(feature = "egui", feature = "winit"))]
        if self.enabled_sinks.contains(&Sink::Egui) && self.enabled_sinks.contains(&Sink::Winit) {
            return Err(cmd.error(
                ErrorKind::ArgumentConflict,
                "the egui and winit sinks can not be enabled at the same time",
            ));
        }

        Ok(())
    }
}
