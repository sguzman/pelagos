use std::{
    io::{self, IsTerminal, Read},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use clap::{Parser, Subcommand};
use pelagos::{app, discovery, identity};
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

    /// Enable opt-in Jellyfin UDP discovery on port 7359 (requires LAN bind).
    #[arg(long)]
    discover: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read-only inventory of a local media folder (does not start the HTTP server).
    Scan {
        /// Directory to inspect recursively. Symlinks are not followed.
        directory: PathBuf,
    },
    /// Synchronize one read-only filesystem scan into Pelagos' private SQLite index.
    Index { directory: PathBuf },
    /// Create a local Pelagos user, reading the password from standard input only.
    UserAdd { username: String },
    /// Allow an existing local user to read one explicitly named media library.
    LibraryGrant {
        username: String,
        directory: PathBuf,
    },
    /// Remove an existing user-library read grant.
    LibraryRevoke {
        username: String,
        directory: PathBuf,
    },
    /// Enable downloading indexed audio/video for an existing account.
    DownloadGrant { username: String },
    /// Revoke the download privilege from a non-administrator account.
    DownloadRevoke { username: String },
    /// List items in an already indexed library.
    Catalog { directory: PathBuf },
    /// Inspect one media file with the installed FFprobe executable.
    Probe {
        /// File to examine (never rewritten).
        file: PathBuf,
        /// FFprobe binary path; defaults to the ffprobe executable on PATH.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
    },
}

// A graceful HTTP stop leaves the user's media untouched. The executable
// handles normal interactive SIGINT and service-manager SIGTERM on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::error!(%error, "SIGINT listener failed");
                        }
                    }
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "SIGTERM listener unavailable, falling back to SIGINT");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("shutdown signal received, draining active HTTP responses");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pelagos=info".into()),
        )
        .init();

    let args = Args::parse();
    if let Some(Command::Scan { directory }) = &args.command {
        let report = pelagos::media::scan(directory)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if let Some(Command::Probe { file, ffprobe }) = &args.command {
        let report = pelagos::probe::probe(file, ffprobe, pelagos::probe::DEFAULT_TIMEOUT).await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if let Some(Command::Index { directory }) = &args.command {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let result = pelagos::catalog::index(&state_dir, directory)?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    if let Some(Command::Catalog { directory }) = &args.command {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let result = pelagos::catalog::list(&state_dir, directory)?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    if let Some(Command::UserAdd { username }) = &args.command {
        if io::stdin().is_terminal() {
            return Err(
                "supply a password via standard input (not a command-line argument)".into(),
            );
        }
        let mut input = String::new();
        io::stdin().take(1025).read_to_string(&mut input)?;
        let password = input.trim_end_matches(['\r', '\n']);
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let user = pelagos::auth::create_user(&state_dir, username, password)?;
        println!("created user: {} ({})", user.name, user.id);
        return Ok(());
    }
    if let Some(Command::LibraryGrant {
        username,
        directory,
    }) = &args.command
    {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let created = pelagos::auth::grant_library(&state_dir, username, directory)?;
        println!("library grant created: {created}");
        return Ok(());
    }
    if let Some(Command::LibraryRevoke {
        username,
        directory,
    }) = &args.command
    {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let revoked = pelagos::auth::revoke_library(&state_dir, username, directory)?;
        println!("library grant revoked: {revoked}");
        return Ok(());
    }
    if let Some(Command::DownloadGrant { username }) = &args.command {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let changed = pelagos::auth::set_content_download_permission(&state_dir, username, true)?;
        println!("download privilege created: {changed}");
        return Ok(());
    }
    if let Some(Command::DownloadRevoke { username }) = &args.command {
        let state_dir = args
            .state_dir
            .clone()
            .unwrap_or_else(identity::default_state_dir);
        let changed = pelagos::auth::set_content_download_permission(&state_dir, username, false)?;
        println!("download privilege revoked: {changed}");
        return Ok(());
    }
    let state_dir = args.state_dir.unwrap_or_else(identity::default_state_dir);
    let id = identity::load_or_create(&state_dir)?;
    let state_for_auth = state_dir.clone();
    let address: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let listening_address = listener.local_addr()?;
    let mut discovery_task = None;
    if args.discover {
        if address.ip().is_loopback() {
            return Err("--discover requires an HTTP LAN bind such as --bind 0.0.0.0".into());
        }
        let udp = discovery::bind_default().await?;
        let discovery_id = id.clone();
        discovery_task = Some(tokio::spawn(async move {
            if let Err(error) = discovery::serve(udp, listening_address.port(), discovery_id).await
            {
                tracing::error!(%error, "UDP discovery stopped");
            }
        }));
        info!(
            port = discovery::DISCOVERY_PORT,
            "opt-in Jellyfin UDP discovery active"
        );
    }
    info!(%listening_address, "Pelagos listening (pre-alpha, not yet a usable Jellyfin replacement)");
    let shutdown_requested = Arc::new(tokio::sync::Notify::new());
    let shutdown_notification = shutdown_requested.clone();
    let server = axum::serve(
        listener,
        app::router(Arc::new(
            app::AppState::with_auth_dir(id, state_for_auth).with_listen_addr(listening_address),
        )),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_notification.notify_one();
    });
    tokio::pin!(server);
    let outcome = tokio::select! {
        result = &mut server => result,
        _ = shutdown_requested.notified() => {
            // Slow or abandoned clients cannot hold the daemon forever
            // after an authorized process shutdown signal.
            match tokio::time::timeout(Duration::from_secs(30), &mut server).await {
                Ok(result) => result,
                Err(_) => Err(io::Error::other("HTTP graceful shutdown exceeded 30 seconds")),
            }
        }
    };
    if let Some(task) = discovery_task {
        task.abort();
        let _ = task.await;
    }
    outcome?;
    info!("Pelagos HTTP listener stopped");
    Ok(())
}
