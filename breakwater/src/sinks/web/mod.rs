use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::{Router, extract::ws::Utf8Bytes, routing::get};
use breakwater_parser::{FB_BYTES_PER_PIXEL, FrameBuffer, PixelColorBytes};
use bytes::Bytes;
use color_eyre::eyre::{self, Context, ensure};
use simple_moving_average::{SMA, SingleSumSMA};
use tokio::{
    sync::broadcast,
    time::{self, Instant},
};
use tracing::{info, trace, warn};

use crate::{
    sinks::{
        DisplaySink, DisplaySinkType, Sink,
        web::{
            compression::FrameCompressor,
            state::{ChatHistory, WebState},
        },
    },
    statistics::StatisticsInformationEvent,
};

pub use cli_args::WebSinkCliArgs;

mod cli_args;
mod compression;
mod http_api;
mod state;

/// Number of recent frames over which the average compression duration is computed for logging.
const COMPRESSION_TIME_WINDOW_SIZE: usize = 100;

/// Number of compressed frames buffered for each connected client. Kept small on purpose:
/// a client that can't drain the buffer in time receives a [`broadcast::error::RecvError::Lagged`]
/// and simply skips ahead to the newest frame, which reduces its effective frame rate.
const FRAME_BUFFER_SIZE: usize = 2;

/// Number of stats messages buffered per client. Stats are produced roughly once per second, so a
/// small buffer is plenty; a client that lags simply skips the missed updates. As of writing no
/// data is lost in case a stats message is missed, so we don't need to be super careful about that.
const STATS_BUFFER_SIZE: usize = 3;

/// Number of chat messages buffered per client. A client that lags this far behind will miss some
/// chat messages. As they should be cheap to send we try to deliver all of them.
const CHAT_BUFFER_SIZE: usize = 1024;

pub struct WebSink<FB: FrameBuffer> {
    listen_addresses: Vec<SocketAddr>,
    fb: Arc<FB>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    terminate_signal_rx: broadcast::Receiver<()>,

    fps: u32,

    /// Shared state handed to every connection handler (channels, rate limiter, canvas size, ...).
    /// The sink keeps its own copy to feed the encoder loop and stats task.
    state: WebState,

    /// Reused scratch buffer holding one RGBA frame, so we don't reallocate every tick. Kept in an
    /// [`Arc`], as the compression tasks need to share it (see [`WebSink::encode_frame`]).
    frame_buf: Arc<Vec<u8>>,
    frame_compressor: FrameCompressor,
    frame_compression_chunks: usize,

    /// Rolling average of the per-frame compression duration (in microseconds), logged alongside
    /// the instantaneous duration to give a more stable picture.
    compression_time_window: SingleSumSMA<u64, u64, COMPRESSION_TIME_WINDOW_SIZE>,
}

impl<FB: FrameBuffer + PixelColorBytes + Sync + Send> WebSink<FB> {
    pub fn new(
        fb: Arc<FB>,
        WebSinkCliArgs {
            web_listen_addresses,
            frame_compression_level,
            frame_compression_chunks,
            chat_messages_per_minute,
        }: &WebSinkCliArgs,
        advertised_endpoints: Vec<String>,
        fps: u32,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Self> {
        ensure!(
            !web_listen_addresses.is_empty(),
            "the web sink needs at least one --web-listen-address to listen on",
        );

        let (frame_tx, _) = broadcast::channel(FRAME_BUFFER_SIZE);
        let (stats_tx, _) = broadcast::channel(STATS_BUFFER_SIZE);
        let (chat_tx, _) = broadcast::channel(CHAT_BUFFER_SIZE);
        let frame_buf = Arc::new(vec![0; fb.get_size() * FB_BYTES_PER_PIXEL]);

        let state = WebState {
            frame_tx,
            stats_tx,
            chat_tx,
            chat_rate_limit: *chat_messages_per_minute,
            chat_rate_limiter: Arc::new(Mutex::new(HashMap::new())),
            chat_history: ChatHistory::default(),
            width: fb.get_width(),
            height: fb.get_height(),
            advertised_endpoints,
        };

        Ok(Self {
            listen_addresses: web_listen_addresses.clone(),
            fb,
            statistics_information_rx,
            terminate_signal_rx,
            fps,
            state,
            frame_buf,
            frame_compressor: FrameCompressor::new(*frame_compression_level)?,
            frame_compression_chunks: *frame_compression_chunks,
            compression_time_window: SingleSumSMA::new(),
        })
    }
}

impl<FB: FrameBuffer + PixelColorBytes + Sync + Send> DisplaySinkType<FB> for WebSink<FB> {
    fn sink_type() -> Sink {
        Sink::Web
    }
}

#[async_trait]
impl<FB: FrameBuffer + PixelColorBytes + Sync + Send> DisplaySink<FB> for WebSink<FB> {
    async fn run(&mut self) -> eyre::Result<()> {
        let state = self.state.clone();

        // Dedicated task: serialize every incoming statistics event to JSON (once, not per client)
        // and broadcast it. The full per-IP maps are included so the frontend can build show
        // traffic per IP.
        let mut statistics_information_rx = self.statistics_information_rx.resubscribe();
        let stats_tx = self.state.stats_tx.clone();
        let stats_task = tokio::spawn(async move {
            loop {
                match statistics_information_rx.recv().await {
                    Ok(info) => match serde_json::to_value(&info) {
                        Ok(mut value) => {
                            if let Some(object) = value.as_object_mut() {
                                object.insert("type".to_owned(), "stats".into());
                            }
                            // Ignore the error: it only means no clients are currently connected.
                            let _ = stats_tx.send(Utf8Bytes::from(value.to_string()));
                        }
                        Err(err) => warn!(%err, "failed to serialize statistics to JSON"),
                    },
                    // We fell behind on statistics events; just continue with the next one.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    // The statistics thread shut down, so will we.
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let app = Router::new()
            .route("/", get(http_api::index))
            .route("/ws", get(http_api::ws_handler))
            .with_state(state);

        // One HTTP server per listen address. They all share the same router - and therefore the
        // same frame, statistics and chat channels - so it doesn't matter which one a client uses.
        let mut servers = Vec::with_capacity(self.listen_addresses.len());
        for &listen_address in &self.listen_addresses {
            let listener = tokio::net::TcpListener::bind(listen_address)
                .await
                .with_context(|| format!("failed to bind web server to {listen_address}"))?;
            info!(
                "Web UI available at http://{}",
                listener.local_addr().unwrap_or(listen_address)
            );

            let app = app.clone();
            // We deliberately don't use axum's `with_graceful_shutdown` here: it waits for all open
            // connections, so a single client that opened a connection without ever completing its
            // request would keep us from ever shutting down. There is also nothing worth draining, as
            // the only plain HTTP response we serve is the (small and static) index page. So we just
            // abort these tasks once the encoder loop below is done.
            // Note that websocket connections would *not* hold a graceful shutdown up, as hyper stops
            // tracking a connection once it has been upgraded.
            servers.push(tokio::spawn(async move {
                // `into_make_service_with_connect_info` makes the peer `SocketAddr` available to
                // handlers via `ConnectInfo`, which we use for the per-IP chat rate limit.
                if let Err(err) = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                {
                    warn!(%err, %listen_address, "web server stopped unexpectedly");
                }
            }));
        }

        // Note that we don't use `?` here, as we need to clean up the tasks we spawned above even if
        // the encoder loop fails.
        let result = self.encode_loop().await;

        // We are terminating, so stop serving. This drops all open connections without sending a
        // websocket close frame, which is fine: clients treat that the same as any other connection
        // loss and simply try to reconnect.
        for server in servers {
            server.abort();
        }
        stats_task.abort();

        result
    }
}

impl<FB: FrameBuffer + PixelColorBytes> WebSink<FB> {
    /// Compresses the framebuffer once per frame and broadcasts the bytes to every connected client,
    /// until we are asked to terminate. The expensive work (copy + compress) happens a single time
    /// regardless of the number of viewers.
    async fn encode_loop(&mut self) -> eyre::Result<()> {
        let mut interval = time::interval(Duration::from_micros(1_000_000 / u64::from(self.fps)));
        // In case we delayed a frame, there is no point in trying to get the following frames
        // quicker as a compensation.
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        loop {
            if self.terminate_signal_rx.try_recv().is_ok() {
                break;
            }

            // No point spending CPU on compression while nobody is watching.
            if self.state.frame_tx.receiver_count() > 0 {
                match self.encode_frame().await {
                    Ok(frame) => {
                        // Ignore the error: it only means all receivers disconnected between the
                        // check above and here.
                        let _ = self.state.frame_tx.send(frame);
                    }
                    // A frame we can't encode costs the spectators exactly that one frame, so we
                    // skip it and carry on with the next. Notably we do *not* propagate the error:
                    // returning it ends the sink, and a sink ending brings the whole Pixelflut
                    // server down with it (see `start_sinks`).
                    Err(err) => warn!(?err, "failed to encode a frame for the web sink"),
                }
            }

            interval.tick().await;
        }

        Ok(())
    }

    /// Copies the current framebuffer into the scratch buffer, forces the alpha channel to opaque
    /// (the framebuffer stores `rgb0`, but the browser's `ImageData` expects a meaningful alpha),
    /// and zlib-compresses the result.
    ///
    /// Compression is the single most expensive part of serving the web UI, so the buffer is split
    /// into contiguous byte ranges that are compressed in parallel. The compressed chunks are
    /// concatenated into one message, prefixed with a small header so the client can split them
    /// apart again:
    ///
    /// ```text
    /// u32le  chunk_count
    /// u32le  compressed_len   × chunk_count
    /// bytes  compressed chunk data, back-to-back, in order
    /// ```
    ///
    /// Because the chunks are simply consecutive slices of the framebuffer, the client reproduces
    /// the full frame by decompressing each chunk and concatenating the output in order. Keeping
    /// everything in a single websocket message guarantees a client never renders a half-updated
    /// frame (which would show as a visible tear/artefact).
    async fn encode_frame(&mut self) -> eyre::Result<Bytes> {
        // `make_mut` hands us exclusive access to the buffer. It would only need to clone it in case
        // a compression task of a previous frame was still running, which can't happen as we always
        // await all of them below.
        let frame_buf = Arc::make_mut(&mut self.frame_buf);
        frame_buf.copy_from_slice(self.fb.pixel_color_bytes());
        for pixel in frame_buf.as_chunks_mut::<FB_BYTES_PER_PIXEL>().0 {
            pixel[3] = 0xff;
        }

        let frame_compressor = self.frame_compressor;

        let start = Instant::now();

        let len = self.frame_buf.len();
        // Round up so we never end up with more than `frame_compression_chunks` chunks. The exact
        // split points don't matter for correctness as the client reassembles the chunks in order.
        let chunk_size = len.div_ceil(self.frame_compression_chunks).max(1);

        // Compress each chunk on Tokio's blocking thread pool. `spawn_blocking` requires `'static`
        // closures, so every task gets its own handle to the shared scratch buffer.
        let mut tasks = Vec::with_capacity(self.frame_compression_chunks);
        let mut offset = 0;
        while offset < len {
            let end = (offset + chunk_size).min(len);
            let frame = Arc::clone(&self.frame_buf);
            tasks.push(tokio::task::spawn_blocking(move || {
                frame_compressor.compress(&frame[offset..end])
            }));
            offset = end;
        }

        // Collect in spawn order, so the chunks stay in framebuffer order.
        //
        // We await *every* task, even after one of them failed: dropping a `JoinHandle` does not
        // cancel the blocking task behind it, so bailing out early would leave tasks running that
        // still hold a handle to the scratch buffer - and `make_mut` above would have to clone it
        // for the next frame.
        let mut compressed_chunks: Vec<Vec<u8>> = Vec::with_capacity(tasks.len());
        let mut failure = None;
        for task in tasks {
            match task.await.context("compression task panicked") {
                Ok(Ok(chunk)) => compressed_chunks.push(chunk),
                Ok(Err(err)) | Err(err) => failure = failure.or(Some(err)),
            }
        }
        if let Some(err) = failure {
            return Err(err);
        }

        let compression_time = start.elapsed();
        self.compression_time_window
            .add_sample(compression_time.as_micros() as u64);
        let avg_compression_time =
            Duration::from_micros(self.compression_time_window.get_average());

        // Assemble the framed message: header (chunk count + per-chunk lengths) followed by the
        // compressed bytes.
        let compressed_bytes: usize = compressed_chunks.iter().map(Vec::len).sum();
        let header_len = (1 + compressed_chunks.len()) * size_of::<u32>();
        let mut message = Vec::with_capacity(header_len + compressed_bytes);
        message.extend_from_slice(&(compressed_chunks.len() as u32).to_le_bytes());
        for chunk in &compressed_chunks {
            message.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        }
        for chunk in &compressed_chunks {
            message.extend_from_slice(chunk);
        }

        trace!(
            raw_bytes = self.frame_buf.len(),
            compressed_bytes,
            chunks = compressed_chunks.len(),
            compression_factor = self.frame_buf.len() as f64 / compressed_bytes as f64,
            ?compression_time,
            ?avg_compression_time,
            "encoded web frame"
        );

        Ok(Bytes::from(message))
    }
}
