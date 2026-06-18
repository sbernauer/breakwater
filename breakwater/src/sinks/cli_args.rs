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
