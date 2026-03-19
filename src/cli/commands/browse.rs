use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Subcommand};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::client::antenati::AntenatiClient;

#[derive(Debug, Subcommand)]
pub enum BrowseAction {
    /// List all archives (Archivi di Stato) on the portal
    Archives(ArchivesArgs),
}

#[derive(Debug, Args)]
pub struct ArchivesArgs {
    /// Filter archives by name (case-insensitive substring match)
    #[arg(long)]
    pub filter: Option<String>,
}

pub async fn run(
    action: &BrowseAction,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    match action {
        BrowseAction::Archives(args) => run_list_archives(args, json_output, client).await,
    }
}

async fn run_list_archives(
    args: &ArchivesArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    let archives = client.list_archives().await?;

    let filtered: Vec<_> = if let Some(filter) = &args.filter {
        let filter_lower = filter.to_lowercase();
        archives
            .into_iter()
            .filter(|a| a.name.to_lowercase().contains(&filter_lower))
            .collect()
    } else {
        archives
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&filtered)?);
        return Ok(());
    }

    eprintln!("{} archives found.", filtered.len());

    if filtered.is_empty() {
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Archive", "Slug"]);

    for (i, a) in filtered.iter().enumerate() {
        table.add_row(vec![&format!("{}", i + 1), &a.name, &a.slug]);
    }

    println!("{table}");
    Ok(())
}
