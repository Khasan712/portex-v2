use clap::Parser;
use tracing_subscriber::EnvFilter;

mod args;
mod config;
mod local;
mod tunnel;

use args::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_target(false)
        .init();

    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    match cli.command {
        Command::Auth { token } => config::store_token(&token),
        Command::Http(opts) => tunnel::run(opts).await,
    }
}
