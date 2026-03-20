use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use comfy_table::{Cell, Table};

use crate::client::antenati::AntenatiClient;
use crate::download::state::StateDb;
use crate::models::manifest::IiifManifest;

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Manifest URL, archive ID, or ARK identifier
    pub source: String,

    /// Show full manifest details including all canvases
    #[arg(long)]
    pub full: bool,
}

pub async fn run(
    args: &InfoArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
    state_db: Option<&StateDb>,
) -> Result<()> {
    let manifest_url = client.resolve_manifest_url(&args.source).await?;
    let manifest = client.get_manifest(&manifest_url).await?;

    // Persist manifest metadata to local database
    if let Some(db) = state_db {
        if let Err(e) = db.store_manifest_from_iiif(&manifest, Some(&args.source)) {
            tracing::warn!("Failed to cache manifest metadata: {e}");
        }
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        return Ok(());
    }

    print_manifest_info(&manifest, args.full);
    Ok(())
}

fn print_manifest_info(manifest: &IiifManifest, full: bool) {
    println!("IIIF Manifest ({})", manifest.version);
    println!("─────────────────────────────────────────");
    println!("ID:       {}", manifest.id);
    println!("Label:    {}", manifest.label);
    println!("Canvases: {}", manifest.canvases.len());

    if !manifest.metadata.is_empty() {
        println!();
        println!("Metadata:");
        for entry in &manifest.metadata {
            println!("  {}: {}", entry.label, entry.value);
        }
    }

    if full && !manifest.canvases.is_empty() {
        println!();
        let mut table = Table::new();
        table.set_header(vec!["#", "Label", "Width", "Height", "Image Service"]);

        for (i, canvas) in manifest.canvases.iter().enumerate() {
            let service_short = if canvas.image_service.id.len() > 60 {
                format!("...{}", &canvas.image_service.id[canvas.image_service.id.len() - 57..])
            } else {
                canvas.image_service.id.clone()
            };

            table.add_row(vec![
                Cell::new(i + 1),
                Cell::new(&canvas.label),
                Cell::new(canvas.width),
                Cell::new(canvas.height),
                Cell::new(service_short),
            ]);
        }

        println!("{table}");
    }
}
