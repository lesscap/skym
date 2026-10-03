//! `skym-server`: serves the API, or prints a new token.

use clap::{Parser, Subcommand};
use skym_server::api::{AppState, router};
use skym_server::{config, db, store::Store, tasks};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "skym-server", version, about = "Receives skym reports and serves the query API")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API
    Serve {
        #[arg(long, default_value = "/etc/skym-server/config.toml")]
        config: PathBuf,
    },
    /// Print a new token and the hash to put in the configuration
    Token,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            // The verifier logs every certificate it rejects; a probe reports that as an incident.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rustls_platform_verifier=off".into()),
        )
        .init();
    match Cli::parse().command {
        Command::Token => {
            let (token, hash) = config::new_token()?;
            println!("token:        {token}\ntoken_sha256: {hash}");
            Ok(())
        }
        Command::Serve { config } => serve(&config).await,
    }
}

async fn serve(path: &std::path::Path) -> anyhow::Result<()> {
    let cfg = config::load(path)?;
    let store = Store::new(db::open(&cfg.database)?);
    let listen = cfg.listen;
    let state = AppState::new(store, cfg, jiff::Timestamp::now());
    tasks::spawn(state.store.clone(), state.cfg.clone(), state.started);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!("listening on {listen}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
