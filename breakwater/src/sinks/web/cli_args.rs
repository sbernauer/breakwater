use std::net::SocketAddr;

#[derive(Clone, Debug, clap::Args)]
#[command(next_help_heading = "web sink options")]
pub struct WebSinkCliArgs {
    /// Web server listen address to bind to (multiple can be specified).
    /// The default value will listen on all interfaces for IPv4 and IPv6 packets.
    //
    // We can't call it listen_addresses because of
    // Command breakwater: Argument names must be unique, but 'listen_addresses' is in use by more than one argument or group
    #[clap(long = "web-listen-address", default_value = "[::]:8080")]
    pub web_listen_addresses: Vec<SocketAddr>,

    /// The zlib compression level to use to compress frames before sending them to the connected clients.
    ///
    /// The compression level goes from 0 (no compression) to 9 (maximum compression).
    ///
    /// Pixelflut battles are high-entropy, so I don't recommend high compression levels: They burn
    /// much CPU for very little compression gains. Measure yourself! Here are some highly
    /// sophisticated (not) measurements I did on my Laptop using Chrome to stream a static
    /// 1920 x 1080 image. They show the traffic and CPU usage at the given compression levels.
    ///
    /// level 0: 1990 Mbit/s @ 193% CPU usage,
    /// level 1: 795  Mbit/s @ 203% CPU usage,
    /// level 2: 661  Mbit/s @ 288% CPU usage,
    /// level 3: 639  Mbit/s @ 403% CPU usage,
    /// level 5: 632  Mbit/s @ 481% CPU usage,
    /// level 9: 627  Mbit/s @ 602% CPU usage
    #[clap(
        long = "web-frame-compression-level",
        default_value_t = 2,
        value_parser = clap::value_parser!(u32).range(0..=9),
    )]
    pub frame_compression_level: u32,

    /// The number of independently-compressed chunks a frame is split into before compression.
    ///
    /// The framebuffer is split into this many contiguous byte ranges that are zlib-compressed in
    /// parallel, drastically cutting the wall-clock time spent compressing a single frame. All
    /// chunks are packed into one websocket message, so a client never renders a partially-updated
    /// frame.
    ///
    /// Note: If you change this value, the frontend adapts automatically as the chunk count is encoded
    /// in the message header.
    #[clap(
        long = "web-frame-compression-chunks",
        default_value_t = 16,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=512),
    )]
    pub frame_compression_chunks: usize,

    /// Maximum number of chat messages a single IP address may send per minute in the WebUI.
    #[clap(long = "web-chat-messages-per-minute", default_value_t = 10)]
    pub chat_messages_per_minute: u32,
}
