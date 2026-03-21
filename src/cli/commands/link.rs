use anyhow::Result;
use clap::{Args, Subcommand};

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum LinkAction {
    /// Find potential cross-record matches (same person across birth/marriage/death registries)
    Find(FindArgs),

    /// Show linked records for a person
    Show(ShowArgs),
}

#[derive(Debug, Args)]
pub struct FindArgs {
    /// Minimum confidence threshold for matches (0.0-1.0)
    #[arg(long, default_value = "0.5")]
    pub threshold: f64,

    /// Only process a specific manifest
    #[arg(long)]
    pub manifest: Option<String>,

    /// Maximum number of matches to display
    #[arg(long, default_value = "50")]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Person ID to show linked records for
    pub person_id: i64,
}

pub fn run(action: &LinkAction, json_output: bool) -> Result<()> {
    let state_db = StateDb::open(&output::db_path())?;

    match action {
        LinkAction::Find(args) => find_links(&state_db, args, json_output),
        LinkAction::Show(args) => show_links(&state_db, args, json_output),
    }
}

/// Cross-record linking strategy:
/// 1. Extract surname + given_name tags from OCR results
/// 2. Group by (normalized_surname, normalized_given_name)
/// 3. For groups spanning multiple manifests/doc_types, create potential links
/// 4. Score by matching additional fields: year proximity, locality match
fn find_links(db: &StateDb, args: &FindArgs, json_output: bool) -> Result<()> {
    // Query: find tags of type 'surname' and 'name' grouped by download_id
    let candidates = db.find_cross_record_candidates(args.threshold, args.limit)?;

    if candidates.is_empty() {
        eprintln!("No cross-record matches found.");
        return Ok(());
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&candidates)?);
    } else {
        eprintln!("Found {} potential cross-record links:\n", candidates.len());
        for (i, c) in candidates.iter().enumerate() {
            println!(
                "{:3}. {} {} — {} records across {} registries (score: {:.2})",
                i + 1,
                c.surname,
                c.given_name,
                c.record_count,
                c.manifest_count,
                c.score,
            );
            for r in &c.records {
                println!(
                    "      {} {} [{}] canvas {}",
                    r.doc_type.as_deref().unwrap_or("?"),
                    r.year.as_deref().unwrap_or("?"),
                    r.manifest_id,
                    r.canvas_id,
                );
            }
        }
    }

    Ok(())
}

fn show_links(db: &StateDb, args: &ShowArgs, json_output: bool) -> Result<()> {
    let records = db.get_person_records(args.person_id)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        if records.is_empty() {
            eprintln!("No records found for person ID {}", args.person_id);
            return Ok(());
        }
        println!("Records for person #{}:", args.person_id);
        for r in &records {
            println!(
                "  {} {} — {} ({})",
                r.record_type.as_deref().unwrap_or("?"),
                r.date.as_deref().unwrap_or("?"),
                r.ark_url.as_deref().unwrap_or("?"),
                r.manifest_id.as_deref().unwrap_or("?"),
            );
        }
    }

    Ok(())
}
