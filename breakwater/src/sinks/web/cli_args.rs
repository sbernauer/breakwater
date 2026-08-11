use std::net::SocketAddr;

#[derive(Clone, Debug, clap::Args)]
#[command(next_help_heading = "web sink options")]
pub struct WebSinkCliArgs {
    /// Web server listen address to bind to (multiple can be specified).
    //
    // We can't call it listen_addresses because of
    // Command breakwater: Argument names must be unique, but 'listen_addresses' is in use by more than one argument or group
    #[clap(long = "web-listen-address")]
    pub web_listen_addresses: Vec<SocketAddr>,

    /// Maximum number of chat messages a single IP address may send per minute in the WebUI.
    #[clap(long = "web-chat-messages-per-minute", default_value_t = 10)]
    pub chat_messages_per_minute: u32,
}

impl WebSinkCliArgs {
    /// Validates the arguments under the assumption that the web sink is enabled.
    pub fn validate(&self) -> Result<(), String> {
        if self.web_listen_addresses.is_empty() {
            return Err(
                "the web sink requires at least one '--web-listen-address' to be specified"
                    .to_owned(),
            );
        }

        Ok(())
    }
}
