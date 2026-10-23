use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use clap::Parser;
use pelagos::{app, identity};
use tracing::info;

#[derive(Debug, Parser)]
#[command(about = "Pelagos: a Rust-first Jellyfin-compatible media server (early scaffold)")]
struct Args {
    /// Bind address. Default is loopback; use 0.0.0.0 explicitly for LAN discovery.
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// HTTP port, separate from Jellyfin's default port 8096.
    #[arg(long, default_value_t = 8097)]
    port: u16,

    /// Private state directory (defaults to XDG_STATE_HOME/pelagos).
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pelagos=info".into()),
        )
        .init();

    let args = Args::parse();
    let state_dir = args.state_dir.unwrap_or_else(identity::default_state_dir);
    let id = identity::load_or_create(&state_dir)?;
    let address: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(%address, "Pelagos listening (pre-alpha, not yet a usable Jellyfin replacement)");
    axum::serve(listener, app::router(Arc::new(app::AppState::new(id)))).await?;
    Ok(())
}
