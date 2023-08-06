use std::{process::Stdio, sync::Arc, time::Duration};

use chrono::Local;
use tokio::{io::AsyncWriteExt, process::Command, sync::oneshot::Receiver, time};

use crate::{args::Args, framebuffer::FrameBuffer};

pub struct FfmpegSink {
    fb: Arc<FrameBuffer>,
    rtmp_address: Option<String>,
    video_save_folder: Option<String>,
    fps: u32,
}

impl FfmpegSink {
    pub fn new(args: &Args, fb: Arc<FrameBuffer>) -> Option<Self> {
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

    pub async fn run<'a>(
        &self,
        mut terminate_signal_rx: Receiver<&'a str>,
    ) -> tokio::io::Result<()> {
        let mut ffmpeg_args: Vec<String> = self
            .ffmpeg_input_args()
            .into_iter()
            .flat_map(|(arg, value)| [format!("-{arg}"), value])
            .collect();

        match &self.rtmp_address {
            Some(rtmp_address) => match &self.video_save_folder {
                Some(video_save_folder) => {
                    ffmpeg_args.extend(
                        self.ffmpeg_rtmp_sink_args()
                            .into_iter()
                            .flat_map(|(arg, value)| [format!("-{arg}"), value])
                            .collect::<Vec<_>>(),
                    );
                    let video_file = format!(
                        "{video_save_folder}/pixelflut_dump_{}.mp4",
                        Local::now().format("%Y-%m-%d_%H-%M-%S")
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
                            rtmp_address = rtmp_address.clone(),
                        ),
                    ]);
                    todo!("Writing to file and rtmp sink simultaneously currently not supported");
                }
                None => {
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
                Some(video_save_folder) => {
                    let video_file = format!(
                        "{video_save_folder}/pixelflut_dump_{}.mp4",
                        Local::now().format("%Y-%m-%d_%H-%M-%S")
                    );
                    ffmpeg_args.extend([video_file])
                }
                None => unreachable!(
                    "FfmpegSink can only be created when either rtmp or video file is activated"
                ),
            },
        }

        log::info!("ffmpeg {}", ffmpeg_args.join(" "));
        let mut command = Command::new("ffmpeg")
            .kill_on_drop(false)
            .args(ffmpeg_args)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdin = command
            .stdin
            .take()
            .expect("child did not have a handle to stdin");

        let mut interval = time::interval(Duration::from_micros(1_000_000 / 30));
        loop {
            if terminate_signal_rx.try_recv().is_ok() {
                command.kill().await?;
                return Ok(());
            }
            let bytes = self.fb.as_bytes();
            stdin.write_all(bytes).await?;
            interval.tick().await;
        }
    }

    fn ffmpeg_input_args(&self) -> Vec<(String, String)> {
        let video_size: String = format!("{}x{}", self.fb.get_width(), self.fb.get_height());
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
}
