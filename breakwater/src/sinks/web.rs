use std::{io::Write, net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::{Html, Response},
    routing::get,
};
use breakwater_parser::{FB_BYTES_PER_PIXEL, FrameBuffer};
use bytes::Bytes;
use color_eyre::eyre::{self, Context};
use flate2::{Compression, write::ZlibEncoder};
use tokio::{
    sync::{broadcast, mpsc},
    time,
};
use tracing::{debug, instrument, trace, warn};

use crate::{
    cli_args::CliArgs,
    sinks::DisplaySink,
    statistics::{StatisticsEvent, StatisticsInformationEvent},
};

const INDEX_HTML: &str = include_str!("web_index.html");

/// Number of compressed frames buffered for each connected client. Kept small on purpose:
/// a client that can't drain the buffer in time receives a [`broadcast::error::RecvError::Lagged`]
/// and simply skips ahead to the newest frame, which reduces its effective frame rate.
const FRAME_BUFFER_SIZE: usize = 2;

/// Number of stats messages buffered per client. Stats are produced roughly once per second, so a
/// small buffer is plenty; a client that lags simply skips the missed updates. As of writing no
/// data is lost in case a stats message is missed, so we don't need to be super careful about that.
const STATS_BUFFER_SIZE: usize = 3;

#[derive(Clone)]
struct WebState {
    /// Carries the latest frame already serialized to binary BLOB, ready to send to every client.
    frame_tx: broadcast::Sender<Bytes>,
    /// Carries the latest statistics already serialized to JSON, ready to send to every client.
    stats_tx: broadcast::Sender<Utf8Bytes>,
    width: usize,
    height: usize,
    /// Pixelflut endpoints to advertise to users, sent once on connect.
    advertised_endpoints: Vec<String>,
}

pub struct WebSink<FB: FrameBuffer> {
    fb: Arc<FB>,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    terminate_signal_rx: broadcast::Receiver<()>,

    listen_address: SocketAddr,
    fps: u32,
    frame_tx: broadcast::Sender<Bytes>,
    stats_tx: broadcast::Sender<Utf8Bytes>,
    advertised_endpoints: Vec<String>,

    /// Reused scratch buffer holding one RGBA frame, so we don't reallocate every tick.
    frame_buf: Vec<u8>,
}

#[async_trait]
impl<FB: FrameBuffer + Sync + Send + 'static> DisplaySink<FB> for WebSink<FB> {
    #[instrument(skip_all, err)]
    async fn new(
        fb: Arc<FB>,
        cli_args: &CliArgs,
        _statistics_tx: mpsc::Sender<StatisticsEvent>,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
        terminate_signal_rx: broadcast::Receiver<()>,
    ) -> eyre::Result<Option<Self>> {
        let Some(listen_address) = cli_args.web_listen_address else {
            debug!("Web sink not enabled as no web listen address is specified");
            return Ok(None);
        };

        let (frame_tx, _) = broadcast::channel(FRAME_BUFFER_SIZE);
        let (stats_tx, _) = broadcast::channel(STATS_BUFFER_SIZE);
        let frame_buf = vec![0; fb.get_size() * FB_BYTES_PER_PIXEL];

        Ok(Some(Self {
            fb,
            statistics_information_rx,
            terminate_signal_rx,
            listen_address,
            fps: cli_args.fps,
            frame_tx,
            stats_tx,
            advertised_endpoints: cli_args.resolve_advertised_endpoints(),
            frame_buf,
        }))
    }

    #[instrument(skip(self), err)]
    async fn run(&mut self) -> eyre::Result<()> {
        let state = WebState {
            frame_tx: self.frame_tx.clone(),
            stats_tx: self.stats_tx.clone(),
            width: self.fb.get_width(),
            height: self.fb.get_height(),
            advertised_endpoints: self.advertised_endpoints.clone(),
        };

        // Dedicated task: serialize every incoming statistics event to JSON (once, not per client)
        // and broadcast it. The full per-IP maps are included so the frontend can build show
        // traffic per IP.
        let mut statistics_information_rx = self.statistics_information_rx.resubscribe();
        let stats_tx = self.stats_tx.clone();
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
            .route("/", get(index))
            .route("/ws", get(ws_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(self.listen_address)
            .await
            .with_context(|| format!("failed to bind web server to {}", self.listen_address))?;
        tracing::info!(
            "Web UI available at http://{}",
            listener.local_addr().unwrap_or(self.listen_address)
        );

        // Shut the HTTP server down gracefully once we receive the terminate signal.
        let mut server_terminate_rx = self.terminate_signal_rx.resubscribe();
        let server = tokio::spawn(async move {
            let shutdown = async move {
                let _ = server_terminate_rx.recv().await;
            };
            if let Err(err) = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await
            {
                warn!(%err, "web server stopped unexpectedly");
            }
        });

        // Encoder loop: compress the framebuffer once per tick and broadcast the bytes to every
        // connected client. The expensive work (copy + compress) happens a single time regardless
        // of the number of viewers.
        let mut interval = time::interval(Duration::from_micros(1_000_000 / u64::from(self.fps)));
        loop {
            if self.terminate_signal_rx.try_recv().is_ok() {
                break;
            }

            // No point spending CPU on compression while nobody is watching.
            if self.frame_tx.receiver_count() > 0 {
                let frame = self.encode_frame()?;
                // Ignore the error: it only means all receivers disconnected between the check above
                // and here.
                let _ = self.frame_tx.send(frame);
            }

            interval.tick().await;
        }

        server.abort();
        stats_task.abort();
        Ok(())
    }
}

impl<FB: FrameBuffer> WebSink<FB> {
    /// Copies the current framebuffer into the scratch buffer, forces the alpha channel to opaque
    /// (the framebuffer stores `rgb0`, but the browser's `ImageData` expects a meaningful alpha),
    /// and zlib-compresses the result.
    fn encode_frame(&mut self) -> eyre::Result<Bytes> {
        self.frame_buf.copy_from_slice(self.fb.as_bytes());
        for pixel in self.frame_buf.chunks_exact_mut(FB_BYTES_PER_PIXEL) {
            pixel[3] = 0xff;
        }

        // `Compression::fast()` (level 1) keeps CPU usage low; Pixelflut battles are high-entropy,
        // so a higher level would mostly burn CPU for little gain.
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&self.frame_buf)
            .context("failed to compress frame")?;
        let compressed = encoder.finish().context("failed to finish compression")?;

        trace!(
            raw_bytes = self.frame_buf.len(),
            compressed_bytes = compressed.len(),
            "encoded web frame"
        );

        Ok(Bytes::from(compressed))
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WebState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: WebState) {
    // Tell the client the canvas dimensions (so it can size the `<canvas>` and allocate
    // `ImageData`) and the Pixelflut endpoints to advertise.
    let hello = serde_json::json!({
        "type": "hello",
        "width": state.width,
        "height": state.height,
        "advertised_endpoints": state.advertised_endpoints,
    })
    .to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    let mut frame_rx = state.frame_tx.subscribe();
    let mut stats_rx = state.stats_tx.subscribe();
    loop {
        tokio::select! {
            frame = frame_rx.recv() => match frame {
                Ok(frame) => {
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        // Client disconnected.
                        break;
                    }
                }
                // This client fell behind: skip the dropped frames and continue with the newest one.
                // This is what throttles slow clients to a lower frame rate.
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    trace!(skipped, "web client lagging behind, dropping frames");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            stats_msg = stats_rx.recv() => match stats_msg {
                Ok(json) => {
                    if socket.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}
