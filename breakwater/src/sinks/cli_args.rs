use clap::{ValueEnum, error::ErrorKind, parser::ValueSource};

use crate::sinks::Sink;

#[derive(clap::Args, Debug)]
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
    /// `enabled_sinks`. So we detect explicitly-passed options via [`ArgMatches::value_source`] and validate manually.
    pub fn validate(
        &self,
        cmd: &mut clap::Command,
        matches: &clap::ArgMatches,
    ) -> Result<(), clap::Error> {
        // (clap arg id [= field name], user-facing flag, sink the option belongs to)
        #[allow(unused_mut)]
        let mut checks: Vec<(&str, &str, Sink)> = vec![
            ("rtmp_address", "--ffmpeg-rtmp-address", Sink::Ffmpeg),
            ("video_save_folder", "--ffmpeg-video-save-folder", Sink::Ffmpeg),
        ];
        #[cfg(feature = "egui")]
        checks.extend([
            ("viewports", "--egui-viewport", Sink::Egui),
            (
                "advertised_endpoints",
                "--egui-advertised-endpoint",
                Sink::Egui,
            ),
            ("ui", "--egui-ui", Sink::Egui),
        ]);
        #[cfg(feature = "ndi")]
        checks.push(("source_name", "--ndi-source-name", Sink::Ndi));
        #[cfg(feature = "vnc")]
        checks.extend([
            ("vnc_listen_addresses", "--vnc-listen-address", Sink::Vnc),
            ("text", "--vnc-text", Sink::Vnc),
            ("font", "--vnc-font", Sink::Vnc),
        ]);

        for (arg_id, flag, sink) in checks {
            let passed = matches.value_source(arg_id) == Some(ValueSource::CommandLine);
            if passed && !self.enabled_sinks.contains(&sink) {
                let sink_name = sink
                    .to_possible_value()
                    .expect("every Sink variant has a name")
                    .get_name()
                    .to_owned();
                return Err(cmd.error(
                    ErrorKind::ArgumentConflict,
                    format!(
                        "the argument '{flag}' can only be used when the '{sink_name}' sink is enabled \
                         (pass '--enable-sink {sink_name}')"
                    ),
                ));
            }
        }

        Ok(())
    }
}
