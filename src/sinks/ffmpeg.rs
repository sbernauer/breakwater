use std::{process::Stdio, sync::Arc, time::Duration};

use chrono::Local;
use tokio::{io::AsyncWriteExt, process::Command, time};

use crate::{args::Args, framebuffer::FrameBuffer};

pub struct FfmpegSink {
    fb: Arc<FrameBuffer>,
    rtmp_address: Option<String>,
    save_video_to_file: bool,
    fps: u32,
}

impl FfmpegSink {
    pub fn new(args: &Args, fb: Arc<FrameBuffer>) -> Option<Self> {
        if args.rtmp_address.is_some() || args.save_video_to_file {
            Some(FfmpegSink {
                fb,
                rtmp_address: args.rtmp_address.clone(),
                save_video_to_file: args.save_video_to_file,
                fps: args.fps,
            })
        } else {
            None
        }
    }

    pub async fn run(&self) -> tokio::io::Result<()> {
        let mut ffmpeg_args: Vec<String> = self
            .ffmpeg_input_args()
            .into_iter()
            .flat_map(|(arg, value)| [format!("-{arg}"), value])
            .collect();

        let video_file = format!(
            "pixelflut_dump_{}.mp4",
            Local::now().format("%Y-%m-%d_%H-%M-%S")
        );
        match &self.rtmp_address {
            Some(rtmp_address) => {
                if self.save_video_to_file {
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
                            rtmp_address = rtmp_address.clone(),
                        ),
                    ]);
                    todo!("Writing to file and rtmp sink simultaneously currently not supported");
                } else {
                    ffmpeg_args.extend(
                        self.ffmpeg_rtmp_sink_args()
                            .into_iter()
                            .flat_map(|(arg, value)| [format!("-{arg}"), value])
                            .collect::<Vec<_>>(),
                    );
                    ffmpeg_args.extend(["-f".to_string(), "flv".to_string(), rtmp_address.clone()])
                }
            }
            None => {
                if self.save_video_to_file {
                    ffmpeg_args.extend([video_file])
                } else {
                    unreachable!("FfmpegSink can only be created when either rtmp or video file is activated")
                }
            }
        }

        log::info!("ffmpeg {}", ffmpeg_args.join(" "));
        let mut command = Command::new("ffmpeg")
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
            ("b:v", "4500k"),
            ("b:a", "128k"),
            ("threads", "4"),
            // ("f", "flv"),
        ]
        .map(|(s1, s2)| (s1.to_string(), s2.to_string()))
        .into()
    }
}
