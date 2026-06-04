use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::{Html, Response},
    routing::get,
};
use breakwater_parser::{FB_BYTES_PER_PIXEL, FrameBuffer};
use bytes::Bytes;
use color_eyre::eyre::{self, Context};
use flate2::{Compression, write::ZlibEncoder};
use futures::{SinkExt, StreamExt, stream::SplitSink};
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

/// Number of chat messages buffered per client. A client that lags this far behind will miss some
/// chat messages. As they should be cheap to send we try to deliver all of them.
const CHAT_BUFFER_SIZE: usize = 1024;

/// The window over which the per-IP chat rate limit is applied.
const CHAT_RATE_LIMIT_WINDOW: Duration = Duration::from_mins(1);

/// Maximum length (in characters) of a chat username and message. Enforced server-side so a crafted
/// client can't bypass the frontend's `maxlength` and blow up the UI.
///
/// Note: If you change this value, please also change it in the frontend.
const MAX_CHAT_NAME_LEN: usize = 20;
const MAX_CHAT_MESSAGE_LEN: usize = 256;

/// Tracks the timestamps of recent chat messages per IP address, shared across all connections so
/// the rate limit applies per IP rather than per connection.
type ChatRateLimiter = Arc<Mutex<HashMap<IpAddr, VecDeque<Instant>>>>;

#[derive(Clone)]
struct WebState {
    /// Carries the latest frame already serialized to binary BLOB, ready to send to every client.
    frame_tx: broadcast::Sender<Bytes>,
    /// Carries the latest statistics already serialized to JSON, ready to send to every client.
    stats_tx: broadcast::Sender<Utf8Bytes>,
    /// Carries chat messages (already serialized to JSON) to every connected client.
    chat_tx: broadcast::Sender<Utf8Bytes>,
    /// Maximum number of chat messages a single IP may send per [`CHAT_RATE_LIMIT_WINDOW`].
    chat_rate_limit: u32,
    chat_rate_limiter: ChatRateLimiter,
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

    /// Shared state handed to every connection handler (channels, rate limiter, canvas size, ...).
    /// The sink keeps its own copy to feed the encoder loop and stats task.
    state: WebState,

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
        let (chat_tx, _) = broadcast::channel(CHAT_BUFFER_SIZE);
        let frame_buf = vec![0; fb.get_size() * FB_BYTES_PER_PIXEL];

        let state = WebState {
            frame_tx,
            stats_tx,
            chat_tx,
            chat_rate_limit: cli_args.chat_messages_per_minute,
            chat_rate_limiter: Arc::new(Mutex::new(HashMap::new())),
            width: fb.get_width(),
            height: fb.get_height(),
            advertised_endpoints: cli_args.resolve_advertised_endpoints(),
        };

        Ok(Some(Self {
            fb,
            statistics_information_rx,
            terminate_signal_rx,
            listen_address,
            fps: cli_args.fps,
            state,
            frame_buf,
        }))
    }

    #[instrument(skip(self), err)]
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
            // `into_make_service_with_connect_info` makes the peer `SocketAddr` available to
            // handlers via `ConnectInfo`, which we use for the per-IP chat rate limit.
            if let Err(err) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
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
            if self.state.frame_tx.receiver_count() > 0 {
                let frame = self.encode_frame()?;
                // Ignore the error: it only means all receivers disconnected between the check above
                // and here.
                let _ = self.state.frame_tx.send(frame);
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

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(who): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, who.ip(), state))
}

async fn handle_socket(socket: WebSocket, ip: IpAddr, state: WebState) {
    // Split so we can read incoming chat messages and write outgoing frames/stats/chat concurrently.
    let (mut sender, mut receiver) = socket.split();

    // Tell the client the canvas dimensions (so it can size the `<canvas>` and allocate
    // `ImageData`) and the Pixelflut endpoints to advertise.
    let hello = serde_json::json!({
        "type": "hello",
        "width": state.width,
        "height": state.height,
        "advertised_endpoints": state.advertised_endpoints,
    })
    .to_string();
    if sender.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    let mut frame_rx = state.frame_tx.subscribe();
    let mut stats_rx = state.stats_tx.subscribe();
    let mut chat_rx = state.chat_tx.subscribe();
    loop {
        tokio::select! {
            frame = frame_rx.recv() => match frame {
                Ok(frame) => {
                    if sender.send(Message::Binary(frame)).await.is_err() {
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
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            chat_msg = chat_rx.recv() => match chat_msg {
                Ok(json) => {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => handle_incoming_chat(&text, ip, &state, &mut sender).await,
                // Client closed the connection or errored.
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                // Ignore anything else the client might send (binary, ping, pong).
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Parses, validates and rate-limits an incoming chat message. On success it is broadcast to all
/// clients; if the sender hit the rate limit, a `chat_error` is sent back only to them.
async fn handle_incoming_chat(
    text: &str,
    ip: IpAddr,
    state: &WebState,
    sender: &mut SplitSink<WebSocket, Message>,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if value.get("type").and_then(serde_json::Value::as_str) != Some("chat") {
        return;
    }

    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    let message = value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if name.is_empty() || message.is_empty() {
        return;
    }

    // Basic sanity caps so a single message can't blow up the UI.
    let name: String = name.chars().take(MAX_CHAT_NAME_LEN).collect();
    let message: String = message.chars().take(MAX_CHAT_MESSAGE_LEN).collect();

    match check_rate_limit(&state.chat_rate_limiter, ip, state.chat_rate_limit) {
        Ok(()) => {
            let json =
                serde_json::json!({ "type": "chat", "name": name, "text": message, "ip": ip });
            let _ = state.chat_tx.send(Utf8Bytes::from(json.to_string()));
        }
        Err(recent) => {
            let json = serde_json::json!({
                "type": "chat_error",
                "text": format!(
                    "Your IP {ip} already sent {recent} messages in the last minute, limit is {}",
                    state.chat_rate_limit,
                ),
            });
            let _ = sender
                .send(Message::Text(Utf8Bytes::from(json.to_string())))
                .await;
        }
    }
}

/// Records a chat message for `ip` if it is within the per-IP rate limit.
///
/// Returns `Ok(())` if allowed (and records the message), or `Err(recent)` with the number of
/// messages already sent within [`CHAT_RATE_LIMIT_WINDOW`] if the limit has been reached.
fn check_rate_limit(limiter: &ChatRateLimiter, ip: IpAddr, limit: u32) -> Result<(), usize> {
    let now = Instant::now();
    let mut limiter = limiter.lock().expect("chat rate limiter mutex poisoned");
    let timestamps = limiter.entry(ip).or_default();

    // Drop timestamps that have aged out of the window.
    while timestamps
        .front()
        .is_some_and(|&t| now.duration_since(t) > CHAT_RATE_LIMIT_WINDOW)
    {
        timestamps.pop_front();
    }

    if timestamps.len() >= limit as usize {
        Err(timestamps.len())
    } else {
        timestamps.push_back(now);
        Ok(())
    }
}
