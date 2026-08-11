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

    /// Maximum number of chat messages a single IP address may send per minute in the WebUI.
    #[clap(long = "web-chat-messages-per-minute", default_value_t = 10)]
    pub chat_messages_per_minute: u32,
}
