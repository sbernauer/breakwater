//! kleines Filmröllchen’s Shitty Video Streaming (kfsvs)
//!
//! ## Format
//!
//! 24-byte header for each frame:
//! - Fixed prelude, see [`DATA_PRELUDE`].
//! - 4 bytes network-endian frame length (unsigned integer)
//! - 4 bytes framebuffer endianness (1 = little-endian, RGBA order, 0 = big-endian, ABGR order)
//! - raw framebuffer with stride 4, length as per header field

use std::{net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use breakwater_parser::FrameBuffer;
use color_eyre::eyre;
use tokio::{
    io,
    net::UdpSocket,
    sync::broadcast,
    time::{Instant, sleep_until},
};
use tracing::{debug, info, instrument};

use crate::{cli_args::CliArgs, sinks::DisplaySink};

pub struct KfsvsSink<FB: FrameBuffer> {
    fb: Arc<FB>,
    terminate_signal_rx: broadcast::Receiver<()>,

    addr: SocketAddr,
    target_fps: u32,
}

impl<FB: FrameBuffer + Sync + Send + 'static> KfsvsSink<FB> {
    #[instrument(skip_all, err)]
    pub fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>> {
        if let Some(addr) = cli_args.kfsvs {
            Ok(Some(Self {
                fb,
                terminate_signal_rx,
                addr,
                target_fps: cli_args.fps,
            }))
        } else {
            Ok(None)
        }
    }
}

// A basic string of convenient size containing the protocol name and a bunch of data that shouldn’t just appear within a frame.
const DATA_PRELUDE: &[u8; 16] = b"%%KFSVS%%\xaa\xbb\xcc\xdd\xee\xff\0";
const MAX_SAFE_UDP_SIZE: usize = 1280;
const ENDIAN_MARKER: u32 =
    cfg_select! {target_endian = "big" => 0u32, target_endian = "little" => 1u32};

#[async_trait]
impl<FB: FrameBuffer + Sync + Send> DisplaySink<FB> for KfsvsSink<FB> {
    #[instrument(skip(self), err)]
    async fn run(&mut self) -> eyre::Result<()> {
        let socket = UdpSocket::bind("[::]:0").await?;
        socket.connect(self.addr).await?;
        info!(?self.addr, "Connected KFSVS stream");
        let target_wait_time = Duration::from_secs(1) / self.target_fps;
        debug!(
            ?target_wait_time,
            target_fps = self.target_fps,
            "established inter-frame wait time"
        );

        loop {
            if self.terminate_signal_rx.try_recv().is_ok() {
                return eyre::Ok(());
            }
            let start_time = Instant::now();
            let next_start_time = start_time + target_wait_time;
            let res = (async || {
                let mut header: [u8; 24] = [0; _];
                header[0..16].copy_from_slice(DATA_PRELUDE);
                let source_frame_data = self.fb.as_bytes();
                let source_frame_len = source_frame_data.len() as u32;
                header[16..20].copy_from_slice(&source_frame_len.to_be_bytes());
                header[20..].copy_from_slice(&ENDIAN_MARKER.to_be_bytes());
                socket.send(&header).await?;

                let mut remaining_data_slice = source_frame_data;
                while remaining_data_slice.len() > MAX_SAFE_UDP_SIZE {
                    let (safe_slice, remaining) = remaining_data_slice.split_at(MAX_SAFE_UDP_SIZE);
                    remaining_data_slice = remaining;
                    socket.send(safe_slice).await?;
                }
                if remaining_data_slice.len() > 0 {
                    socket.send(remaining_data_slice).await?;
                }

                sleep_until(next_start_time).await;
                eyre::Ok(())
            })()
            .await;
            if let Err(err) = res {
                match err.downcast::<io::Error>() {
                    Ok(inner_e) if inner_e.kind() == io::ErrorKind::ConnectionRefused => {
                        // No receiver yet connected
                        sleep_until(next_start_time).await;
                        continue;
                    }
                    Ok(other) => return Err(other.into()),
                    Err(e) => return Err(e),
                }
            }
        }
    }
}
