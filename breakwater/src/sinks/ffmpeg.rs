use std::{process::Stdio, sync::Arc, time::Duration};

use breakwater_core::framebuffer::FrameBuffer;
use chrono::Local;
use log::debug;
use snafu::{ResultExt, Snafu};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::oneshot::Receiver,
    time::{self},
};

use crate::cli_args::CliArgs;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to start ffmpeg command {command:?}"))]
    StartFfmpeg {
        source: std::io::Error,
        command: String,
    },

    #[snafu(display("Failed to write new data to ffmpeg via stdout"))]
    WriteDataToFfmeg { source: std::io::Error },
}

pub struct FfmpegSink {
    fb: Arc<FrameBuffer>,
    rtmp_address: Option<String>,
    video_save_folder: Option<String>,
    fps: u32,
}

impl FfmpegSink {
    pub fn new(args: &CliArgs, fb: Arc<FrameBuffer>) -> Option<Self> {
        if args.rtmp_address.is_some() || args.video_save_folder.is_some() {
            Some(FfmpegSink {
                fb,
                rtmp_address: args.rtmp_address.clone(),
                video_save_folder: args.video_save_folder.clone(),
                fps: args.fps,
            })
        } else {
            None
        }
    }

    pub async fn run(&self, mut terminate_signal_rx: Receiver<()>) -> Result<(), Error> {
        let mut ffmpeg_args: Vec<String> = self
            .ffmpeg_input_args()
            .into_iter()
            .flat_map(|(arg, value)| [format!("-{arg}"), value])
            .collect();

        match &self.rtmp_address {
            Some(rtmp_address) => match &self.video_save_folder {
                Some(video_save_folder) => {
                    // Write to rtmp and file
                    ffmpeg_args.extend(
                        self.ffmpeg_rtmp_sink_args()
                            .into_iter()
                            .flat_map(|(arg, value)| [format!("-{arg}"), value])
                            .collect::<Vec<_>>(),
                    );
                    ffmpeg_args.extend([
                        "-f".to_string(),
                        "tee".to_string(),
                        "-map".to_string(),
                        "0:v".to_string(),
                        "-map".to_string(),
                        "1:a".to_string(),
                        format!(
                            "{video_file}|[f=flv]{rtmp_address}",
                            video_file = Self::video_file(video_save_folder),
                            rtmp_address = rtmp_address.clone(),
                        ),
                    ]);

                    todo!("Writing to file and rtmp sink simultaneously currently not supported, sorry!");
                }
                None => {
                    // Only write to rtmp
                    ffmpeg_args.extend(
                        self.ffmpeg_rtmp_sink_args()
                            .into_iter()
                            .flat_map(|(arg, value)| [format!("-{arg}"), value])
                            .collect::<Vec<_>>(),
                    );
                    ffmpeg_args.extend(["-f".to_string(), "flv".to_string(), rtmp_address.clone()])
                }
            },
            None => match &self.video_save_folder {
                // Only write to file
                Some(video_save_folder) => {
                    ffmpeg_args.extend([Self::video_file(video_save_folder)])
                }
                None => unreachable!(
                    "FfmpegSink can only be created when either rtmp or video file is activated"
                ),
            },
        }

        let ffmpeg_command = format!("ffmpeg {}", ffmpeg_args.join(" "));
        debug!("Executing {ffmpeg_command:?}");
        let mut command = Command::new("ffmpeg")
            .kill_on_drop(false)
            .args(ffmpeg_args.clone())
            .stdin(Stdio::piped())
            .spawn()
            .context(StartFfmpegSnafu {
                command: ffmpeg_command,
            })?;

        let mut stdin = command
            .stdin
            .take()
            .expect("child did not have a handle to stdin");

        let mut interval = time::interval(Duration::from_micros(1_000_000 / 30));
        loop {
            if terminate_signal_rx.try_recv().is_ok() {
                // Normally we would send SIGINT to ffmpeg and let the process shutdown gracefully and afterwards call
                // `command.wait().await`. Hopever using the `nix` crate to send a `SIGINT` resulted in ffmpeg
                // [2024-05-14T21:35:25Z TRACE breakwater::sinks::ffmpeg] Sending SIGINT to ffmpeg process with pid 58786
                // [out#0/mp4 @ 0x1048740] Error writing trailer: Immediate exit requested
                //
                // As you can see this also corrupted the output mp4 :(
                // So instead we let the process running here and let the kernel clean up (?), which seems to work (?)

                // trace!("Killing ffmpeg process");

                // if cfg!(target_os = "linux") {
                //     if let Some(pid) = command.id() {
                //         trace!("Sending SIGINT to ffmpeg process with pid {pid}");
                //         nix::sys::signal::kill(
                //             nix::unistd::Pid::from_raw(pid.try_into().unwrap()),
                //             nix::sys::signal::Signal::SIGINT,
                //         )
                //         .unwrap();
                //     } else {
                //         error!("The ffmpeg process had no PID, so I could not kill it. Will let tokio kill it instead");
                //         command.start_kill().unwrap();
                //     }
                // } else {
                //     trace!("As I'm not on Linux, YOLO-ing it by letting tokio kill it ");
                //     command.start_kill().unwrap();
                // }

                // let start = Instant::now();
                // command.wait().await.unwrap();
                // trace!("Killied ffmpeg process in {:?}", start.elapsed());

                return Ok(());
            }
            let bytes = self.fb.as_bytes();
            stdin
                .write_all(bytes)
                .await
                .context(WriteDataToFfmegSnafu)?;
            interval.tick().await;
        }
    }

    fn ffmpeg_input_args(&self) -> Vec<(String, String)> {
        let video_size = format!("{}x{}", self.fb.get_width(), self.fb.get_height());
        [
            ("f", "rawvideo"),
            ("pixel_format", "rgb0"),
            ("video_size", video_size.as_str()),
            ("i", "-"),
            ("f", "lavfi"),
            ("i", "anullsrc=channel_layout=stereo:sample_rate=44100"),
        ]
        .map(|(s1, s2)| (s1.to_string(), s2.to_string()))
        .into()
    }

    fn ffmpeg_rtmp_sink_args(&self) -> Vec<(String, String)> {
        [
            ("vcodec", "libx264"),
            ("acodec", "aac"),
            ("pix_fmt", "yuv420p"),
            ("preset", "veryfast"),
            ("r", self.fps.to_string().as_str()),
            ("g", (self.fps * 2).to_string().as_str()),
            ("ar", "44100"),
            ("b:v", "6000k"),
            ("b:a", "128k"),
            ("threads", "4"),
            // ("f", "flv"),
        ]
        .map(|(s1, s2)| (s1.to_string(), s2.to_string()))
        .into()
    }

    fn video_file(video_save_folder: &str) -> String {
        format!(
            "{video_save_folder}/pixelflut_dump_{}.mp4",
            Local::now().format("%Y-%m-%d_%H-%M-%S")
        )
    }
}
