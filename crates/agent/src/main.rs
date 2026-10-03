//! `skym`: checks this host and the containers on it.

mod agent;
mod collect;
mod commands;
mod config;
mod cursors;
mod deliver;
mod doctor;
mod exceptions;
mod facts;
mod memory;
mod outbox;
mod report;
mod status;

use clap::{Parser, Subcommand};
use commands::SchemaKind;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "skym",
    version,
    about = "Agent-first monitoring: checks this host and its containers"
)]
struct Cli {
    /// Configuration file [default: /etc/skym/config.toml]
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What is wrong on this host right now. Exit code: 0 ok, 1 warn, 2 critical, 3 unknown
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Exception groups from container logs, in full detail (nothing leaves the host)
    Exceptions {
        /// Only this workload, as project/service
        #[arg(long)]
        workload: Option<String>,
        /// A duration (15m, 6h, 7d) or an RFC 3339 timestamp
        #[arg(long, default_value = "1h")]
        since: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the report this host would send, without sending it
    Report {
        #[arg(long, required = true)]
        dry_run: bool,
    },
    /// Collect and report every interval (run by systemd)
    Agent,
    /// Check this installation, ending with one real report to the server
    Doctor,
    /// JSON Schema of a command's --json output
    Schema {
        #[arg(value_enum)]
        which: SchemaKind,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // The only TLS provider in the build; installing it cannot conflict.
    if rustls::crypto::ring::default_provider().install_default().is_err() {
        eprintln!("warning: a TLS provider was already installed");
    }
    let result = match config::load(cli.config.as_deref()) {
        Err(e) => Err(e),
        Ok(cfg) => match cli.command {
            Command::Status { json } => commands::status(&cfg, json).await,
            Command::Exceptions { workload, since, json } => {
                commands::exceptions(&cfg, workload.as_deref(), &since, json).await
            }
            Command::Report { .. } => commands::report_dry_run(&cfg).await,
            Command::Schema { which } => commands::schema(which),
            Command::Agent => agent::run(&cfg).await,
            Command::Doctor => doctor::run(&cfg).await,
        },
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(3)
        }
    }
}
