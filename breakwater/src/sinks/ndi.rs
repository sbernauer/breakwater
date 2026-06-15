//! NDI output support.

use std::sync::Arc;

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use color_eyre::eyre::{self, Context, ContextCompat};
use ndi_sdk_sys::{
    four_cc::FourCCVideo,
    frame::video::VideoFrame,
    resolution::Resolution,
    sdk,
    sender::{NDISender, NDISenderBuilder},
};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, instrument, trace};

use crate::{
    cli_args::CliArgs,
    sinks::DisplaySink,
    statistics::{StatisticsEvent, StatisticsInformationEvent},
};

pub struct NdiSink<FB: FrameBuffer> {
    fb: Arc<FB>,
    terminate_signal_rx: broadcast::Receiver<()>,
    target_fps: u32,

    source: Arc<NDISender>,
}

#[async_trait]
impl<FB: FrameBuffer + Sync + Send + 'static> DisplaySink<FB> for NdiSink<FB> {
    #[instrument(skip_all, err)]
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        _statistics_tx: mpsc::Sender<StatisticsEvent>,
        _statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>> {
        if !cli_args.ndi {
            return Ok(None);
        }

        info!(
            version = sdk::version().unwrap_or("NDI SDK version unavailable"),
            "NDI SDK version",
        );
        sdk::initialize().context("failed to initialize NDI SDK")?;

        let source = NDISenderBuilder::new()
            .name(&cli_args.ndi_source_name)?
            .clock_video(true)
            .build()
            .context("failed to build NDI sender")?;

        info!(
            name = source
                .get_source()
                .name()
                .to_str()
                .context("NDI source name is not a valid utf-8 string")?,
            "Started NDI source",
        );

        Ok(Some(Self {
            fb,
            terminate_signal_rx,
            source: Arc::new(source),
            target_fps: cli_args.fps,
        }))
    }

    #[instrument(skip(self), err)]
    async fn run(&mut self) -> eyre::Result<()> {
        let fb = self.fb.clone();
        let mut terminate_signal_rx = self.terminate_signal_rx.resubscribe();
        let source = self.source.clone();
        let target_fps = self.target_fps;

        tokio::task::spawn_blocking(move || {
            let mut frame = VideoFrame::new();
            frame.set_resolution(
                Resolution::try_new(fb.get_width(), fb.get_height())
                    .context("Resolution is not safe for NDI")?,
            )?;
            // The framebuffer is "technically" RGBA, but the alpha values are always zero.
            // If we were to set RGBA here, the image would be entirely black :)
            frame.set_four_cc(FourCCVideo::RGBX)?;
            frame.set_frame_rate((target_fps as i32).into());
            frame
                .try_alloc()
                .context("failed to allocate NDI framebuffer")?;

            loop {
                if terminate_signal_rx.try_recv().is_ok() {
                    return eyre::Ok(());
                }

                let source_frame_data = fb.as_bytes();
                let (target_data, info) = frame
                    .video_data_mut()
                    .context("failed to get mutable access to the NDI frame")?;
                if info.size != source_frame_data.len() {
                    error!(
                        framebuffer_size = source_frame_data.len(),
                        ndi_size = info.size,
                        "Framebuffer size mismatch"
                    );
                    continue;
                }
                target_data.copy_from_slice(source_frame_data);

                // Using async sending would not improve anything, since we run clocked video anyways, in which case async also always blocks.
                // Doing this instead allows us to more easily reuse the frame allocation.
                source
                    .send_video_sync(&frame)
                    .context("failed to send NDI video frame")?;
                trace!(frame = ?frame, "Sent NDI video frame");
            }
        })
        .await
        .context("failed to join NDI sender thread")??;

        Ok(())
    }
}
