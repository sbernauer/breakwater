use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};

use axum::extract::ws::Utf8Bytes;
use bytes::Bytes;
use tokio::{sync::broadcast, time::Instant};

/// Tracks the timestamps of recent chat messages per IP address, shared across all connections so
/// the rate limit applies per IP rather than per connection.
pub type ChatRateLimiter = Arc<Mutex<HashMap<IpAddr, VecDeque<Instant>>>>;

/// The most recent chat messages (already serialized to JSON), shared across all connections.
pub type ChatHistory = Arc<Mutex<VecDeque<Utf8Bytes>>>;

#[derive(Clone)]
pub struct WebState {
    /// Carries the latest frame already serialized to binary BLOB, ready to send to every client.
    pub frame_tx: broadcast::Sender<Bytes>,
    /// Carries the latest statistics already serialized to JSON, ready to send to every client.
    pub stats_tx: broadcast::Sender<Utf8Bytes>,
    /// Carries chat messages (already serialized to JSON) to every connected client.
    pub chat_tx: broadcast::Sender<Utf8Bytes>,
    /// Maximum number of chat messages a single IP may send per chat ratelimit window.
    pub chat_rate_limit: u32,
    pub chat_rate_limiter: ChatRateLimiter,
    /// Replayed to clients when they connect, so that they don't start with an empty chat.
    pub chat_history: ChatHistory,
    pub width: usize,
    pub height: usize,
    /// Pixelflut endpoints to advertise to users, sent once on connect.
    pub advertised_endpoints: Vec<SocketAddr>,
}
