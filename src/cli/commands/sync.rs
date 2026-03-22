use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use crate::client::antenati::AntenatiClient;
use crate::download::state::StateDb;

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Maximum manifests to check per run
    #[arg(long, default_value = "100")]
    pub limit: usize,

    /// Only check manifests older than N days
    #[arg(long)]
    pub older_than_days: Option<u64>,

    /// Dry run: report changes without updating
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(
    args: &SyncArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
    state_db: Option<&StateDb>,
) -> Result<()> {
    let db = state_db.ok_or_else(|| anyhow::anyhow!("State database not available"))?;

    let mut candidates = db.get_sync_candidates()?;

    // Filter by age if requested
    if let Some(days) = args.older_than_days {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
        candidates.retain(|c| c.fetched_at < cutoff_str);
    }

    // Apply limit
    candidates.truncate(args.limit);

    if candidates.is_empty() {
        if json_output {
            println!("{{\"checked\":0,\"updated\":0,\"unchanged\":0,\"errors\":0}}");
        } else {
            eprintln!("No manifests to sync.");
        }
        return Ok(());
    }

    eprintln!("Checking {} manifests for updates...", candidates.len());

    let pb = ProgressBar::new(candidates.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut updated = 0usize;
    let mut unchanged = 0usize;
    let mut errors = 0usize;
    let mut changes: Vec<serde_json::Value> = Vec::new();

    for candidate in &candidates {
        pb.set_message(
            candidate
                .title
                .as_deref()
                .unwrap_or(&candidate.id)
                .chars()
                .take(40)
                .collect::<String>(),
        );

        match client
            .get_manifest_conditional(
                &candidate.id,
                candidate.etag.as_deref(),
                candidate.last_modified.as_deref(),
            )
            .await
        {
            Ok(None) => {
                // Not modified
                unchanged += 1;
            }
            Ok(Some((manifest, etag, last_modified))) => {
                let old_canvases = candidate.total_canvases.unwrap_or(0);
                let new_canvases = manifest.canvases.len();
                let canvas_diff = new_canvases as i64 - old_canvases as i64;

                if !args.dry_run {
                    // Update manifest in DB
                    db.store_manifest_from_iiif(&manifest, None)?;
                    db.update_manifest_sync_headers(
                        &candidate.id,
                        etag.as_deref(),
                        last_modified.as_deref(),
                        Some(new_canvases),
                    )?;
                }

                if json_output {
                    changes.push(serde_json::json!({
                        "manifest_id": candidate.id,
                        "title": candidate.title,
                        "old_canvases": old_canvases,
                        "new_canvases": new_canvases,
                        "canvas_diff": canvas_diff,
                    }));
                } else if canvas_diff != 0 {
                    eprintln!(
                        "  UPDATED: {} ({:+} canvases, {} → {})",
                        candidate.title.as_deref().unwrap_or(&candidate.id),
                        canvas_diff,
                        old_canvases,
                        new_canvases,
                    );
                }

                updated += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to check {}: {e}", candidate.id);
                errors += 1;
            }
        }

        pb.inc(1);
    }

    pb.finish_and_clear();

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checked": candidates.len(),
                "updated": updated,
                "unchanged": unchanged,
                "errors": errors,
                "dry_run": args.dry_run,
                "changes": changes,
            }))?
        );
    } else {
        eprintln!(
            "\nSync complete: {} checked, {} updated, {} unchanged, {} errors{}",
            candidates.len(),
            updated,
            unchanged,
            errors,
            if args.dry_run { " (dry run)" } else { "" },
        );
    }

    Ok(())
}
