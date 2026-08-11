use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, Utf8Bytes, WebSocket},
    },
    response::{Html, Response},
};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use tokio::{sync::broadcast, time::Instant};
use tracing::trace;

use crate::sinks::web::state::{ChatRateLimiter, WebState};

/// Maximum length (in characters) of a chat username and message. Enforced server-side so a crafted
/// client can't bypass the frontend's `maxlength` and blow up the UI.
///
/// Note: If you change this value, please also change it in the frontend.
const MAX_CHAT_NAME_LEN: usize = 20;
const MAX_CHAT_MESSAGE_LEN: usize = 256;

/// The window over which the per-IP chat rate limit is applied.
const CHAT_RATE_LIMIT_WINDOW: Duration = Duration::from_mins(1);

/// Number of chat messages that are kept around and replayed to clients when they connect. Without
/// this the chat would be empty for everybody who didn't watch from the very beginning - including
/// clients that only briefly went away, as we close the websocket while a browser tab is hidden.
const CHAT_HISTORY_LEN: usize = 100;

pub async fn index() -> Html<&'static str> {
    include_str!("index.html").into()
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(who): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
) -> Response {
    // `to_canonical` turns IPv4-mapped IPv6 addresses (which is what we get for legacy IP clients on
    // a dual-stack listener) back into plain IPv4 addresses, just like the Pixelflut server does.
    // Otherwise the same client would show up as `::ffff:1.2.3.4` in the chat, but as `1.2.3.4` in
    // the statistics.
    let ip = who.ip().to_canonical();

    ws.on_upgrade(move |socket| handle_socket(socket, ip, state))
}

async fn handle_socket(socket: WebSocket, ip: IpAddr, state: WebState) {
    // Split so we can read incoming chat messages and write outgoing frames/stats/chat concurrently.
    let (mut sender, mut receiver) = socket.split();

    // Tell the client the canvas dimensions (so it can size the `<canvas>` and allocate
    // `ImageData`), the Pixelflut endpoints to advertise and the IP address we see it as - the
    // client can't know the latter itself, but wants to point out its own traffic in the statistics.
    let hello = serde_json::json!({
        "type": "hello",
        "width": state.width,
        "height": state.height,
        "advertised_endpoints": state.advertised_endpoints,
        "your_ip": ip,
    })
    .to_string();
    if sender.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    let mut frame_rx = state.frame_tx.subscribe();
    let mut stats_rx = state.stats_tx.subscribe();

    // Grab the chat history and subscribe to new messages in one go, i.e. while holding the lock.
    // Otherwise a message sent in between the two would either be missed or shown twice, as
    // `handle_incoming_chat` also records and broadcasts it while holding the lock.
    let (chat_history, mut chat_rx) = {
        let chat_history = state
            .chat_history
            .lock()
            .expect("chat history mutex poisoned");

        (
            chat_history.iter().cloned().collect::<Vec<_>>(),
            state.chat_tx.subscribe(),
        )
    };

    // Replay the recent messages, so that the chat isn't empty for clients that only join now.
    for message in chat_history {
        if sender.send(Message::Text(message)).await.is_err() {
            return;
        }
    }

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
            let serialized = Utf8Bytes::from(json.to_string());

            // Record and broadcast while holding the lock, see `handle_socket` for why.
            let mut chat_history = state
                .chat_history
                .lock()
                .expect("chat history mutex poisoned");
            if chat_history.len() >= CHAT_HISTORY_LEN {
                chat_history.pop_front();
            }
            chat_history.push_back(serialized.clone());

            // Ignore the error: it only means no clients are currently connected.
            let _ = state.chat_tx.send(serialized);
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
