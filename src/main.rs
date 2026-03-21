mod cli;

// Re-export library modules so `cli` commands can use `crate::` paths.
pub use rustenati::client;
pub use rustenati::config;
pub use rustenati::download;
pub use rustenati::error;
pub use rustenati::models;
pub use rustenati::ocr;
pub use rustenati::output;
pub use rustenati::web;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command, SearchMode};
use crate::client::antenati::AntenatiClient;
use crate::download::state::StateDb;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let filter = match cli.verbose {
        0 if cli.quiet => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .with_ansi(!cli.no_color)
        .init();

    // Load config
    let config_path = cli.config.unwrap_or_else(config::Config::default_path);
    let mut config = config::Config::load(&config_path)?;

    // Apply CLI overrides to HTTP config (e.g., --connections)
    if let Command::Download(ref args) = cli.command {
        if let Some(connections) = args.connections {
            config.http.pool_max_idle_per_host = connections;
        }
    }

    // Create HTTP client (shared across commands)
    let client = AntenatiClient::new(&config.http)?;

    // Open shared state database for commands that need it
    let state_db = open_default_state_db();

    // Dispatch commands
    match &cli.command {
        Command::Search { mode } => match mode {
            SearchMode::Name(args) => {
                cli::commands::search::run_name_search(
                    args, cli.json, client.clone(), state_db.as_ref().ok(),
                ).await?;
            }
            SearchMode::Registry(args) => {
                cli::commands::search::run_registry_search(
                    args, cli.json, client.clone(), state_db.as_ref().ok(),
                ).await?;
            }
        },
        Command::Browse { action } => {
            cli::commands::browse::run(action, cli.json, client.clone()).await?;
        }
        Command::Download(args) => {
            cli::commands::download::run(args, cli.json, client.clone()).await?;
        }
        Command::Info(args) => {
            cli::commands::info::run(args, cli.json, client, state_db.as_ref().ok()).await?;
        }
        Command::Ocr(args) => {
            cli::commands::ocr::run(args, cli.json, &config.ocr).await?;
        }
        Command::Tags { action } => {
            cli::commands::tags::run(action, cli.json, &config.ocr).await?;
        }
        Command::Status(args) => {
            cli::commands::status::run(args, cli.json).await?;
        }
        Command::Config { action } => {
            cli::commands::config::run(action, &config)?;
        }
        Command::Query { action } => {
            cli::commands::query::run(action, cli.json)?;
        }
        Command::Serve(args) => {
            cli::commands::serve::run(args).await?;
        }
    }

    Ok(())
}

/// Open the state database at the fixed path ./antenati/rustenati.db.
fn open_default_state_db() -> Result<StateDb> {
    StateDb::open(&output::db_path())
}
