pub mod commands;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "rustenati",
    version,
    about = "High-performance CLI for dumping Italian genealogical records from Portale Antenati",
    long_about = "Rustenati scarica immagini ad alta risoluzione di atti civili (nascita, morte, matrimonio) \
                  dal Portale Antenati (antenati.cultura.gov.it) con supporto per ricerca, \
                  download parallelo, OCR e tagging strutturato."
)]
pub struct Cli {
    /// Config file path
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress output except errors
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Output in JSON format (for scripting)
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search the Portale Antenati
    Search {
        #[command(subcommand)]
        mode: SearchMode,
    },

    /// Browse archives on the portal
    Browse {
        #[command(subcommand)]
        action: commands::browse::BrowseAction,
    },

    /// Download images from a manifest or search result
    Download(commands::download::DownloadArgs),

    /// Inspect a manifest or archive
    Info(commands::info::InfoArgs),

    /// Run OCR on downloaded images
    Ocr(commands::ocr::OcrArgs),

    /// Manage tags extracted from OCR results
    Tags {
        #[command(subcommand)]
        action: commands::tags::TagsAction,
    },

    /// Show download session status
    Status(commands::status::StatusArgs),

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: commands::config::ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum SearchMode {
    /// Search by person name
    Name(commands::search::NameSearchArgs),

    /// Search by registry/archive
    Registry(commands::search::RegistrySearchArgs),
}
