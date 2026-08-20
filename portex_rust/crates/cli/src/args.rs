use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "portex", version, about = "Expose a local server to the internet")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Save an auth token to ~/.portex/config.toml.
    Auth {
        /// Auth token issued by portex.live.
        ///
        /// Tokens are base64url, so roughly one in sixty-four starts with a
        /// hyphen. Without this the parser reads that as a flag and the user
        /// simply cannot save the token they were issued.
        #[arg(allow_hyphen_values = true)]
        token: String,
    },
    /// Expose a local HTTP server.
    Http(HttpOpts),
}

#[derive(Debug, Parser)]
pub struct HttpOpts {
    /// Local port to forward to.
    #[arg(short = 'p', long)]
    pub port: u16,

    /// Subdomain to claim (must be reserved on portex.live).
    #[arg(short = 's', long)]
    pub subdomain: String,

    /// Gateway QUIC endpoint (host:port).
    #[arg(long, env = "PORTEX_SERVER", default_value = "portex.live:4443")]
    pub server: String,

    /// Skip TLS certificate verification (development).
    #[arg(long, env = "PORTEX_INSECURE", default_value_t = false)]
    pub insecure: bool,

    /// Override the saved auth token.
    #[arg(long, env = "PORTEX_TOKEN")]
    pub token: Option<String>,
}
