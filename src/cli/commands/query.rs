use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::download::state::StateDb;

#[derive(Debug, Subcommand)]
pub enum QueryAction {
    /// Search cached registry results
    Registries(RegistriesQueryArgs),

    /// Search persons from name searches
    Persons(PersonsQueryArgs),

    /// Full-text search on OCR results
    Ocr(OcrQueryArgs),

    /// Search manifests in the local database
    Manifests(ManifestsQueryArgs),

    /// Show extended database statistics
    Stats(StatsQueryArgs),
}

#[derive(Debug, Args)]
pub struct RegistriesQueryArgs {
    /// Filter by locality (substring match)
    #[arg(long)]
    pub locality: Option<String>,

    /// Filter by year
    #[arg(long)]
    pub year: Option<String>,

    /// Filter by document type (Nati, Morti, Matrimoni, etc.)
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Filter by archive name (substring match)
    #[arg(long)]
    pub archive: Option<String>,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct PersonsQueryArgs {
    /// Filter by surname (substring match)
    #[arg(long)]
    pub surname: Option<String>,

    /// Filter by given name (substring match)
    #[arg(long)]
    pub name: Option<String>,

    /// Show linked records for each person
    #[arg(long)]
    pub records: bool,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct OcrQueryArgs {
    /// Search text (FTS5 query syntax)
    pub query: String,

    /// Max results
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct ManifestsQueryArgs {
    /// Filter by document type
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Filter by year
    #[arg(long)]
    pub year: Option<String>,

    /// Filter by archive name (substring match)
    #[arg(long)]
    pub archive: Option<String>,

    /// Filter by locality (substring match on archival context)
    #[arg(long)]
    pub locality: Option<String>,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct StatsQueryArgs {
    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

pub fn run(action: &QueryAction, json_output: bool) -> Result<()> {
    match action {
        QueryAction::Registries(args) => run_query_registries(args, json_output),
        QueryAction::Persons(args) => run_query_persons(args, json_output),
        QueryAction::Ocr(args) => run_query_ocr(args, json_output),
        QueryAction::Manifests(args) => run_query_manifests(args, json_output),
        QueryAction::Stats(args) => run_query_stats(args, json_output),
    }
}

fn run_query_registries(args: &RegistriesQueryArgs, json_output: bool) -> Result<()> {
    let db = StateDb::open(&args.db)?;
    let results = db.search_registry_results(
        args.doc_type.as_deref(),
        args.year.as_deref(),
        args.archive.as_deref(),
        args.locality.as_deref(),
    )?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    eprintln!("{} results found.", results.len());

    if results.is_empty() {
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Year", "Type", "Location", "Archive", "ARK", "Downloaded"]);

    for (i, r) in results.iter().enumerate() {
        let locality = r.context.as_deref()
            .and_then(|c| c.rsplit(" > ").next())
            .unwrap_or("-");
        let ark_short = r.ark_url.rsplit('/').next().unwrap_or(&r.ark_url);
        let downloaded = if r.manifest_id.is_some() { "yes" } else { "-" };

        table.add_row(vec![
            &format!("{}", i + 1),
            r.year.as_deref().unwrap_or("-"),
            r.doc_type.as_deref().unwrap_or("-"),
            locality,
            r.archive_name.as_deref().unwrap_or("-"),
            ark_short,
            downloaded,
        ]);
    }

    println!("{table}");
    Ok(())
}

fn run_query_persons(args: &PersonsQueryArgs, json_output: bool) -> Result<()> {
    let db = StateDb::open(&args.db)?;
    let persons = db.search_persons(args.surname.as_deref(), args.name.as_deref())?;

    if json_output {
        if args.records {
            let mut results = Vec::new();
            for p in &persons {
                let records = db.get_person_records(p.id)?;
                results.push(serde_json::json!({
                    "person": p,
                    "records": records,
                }));
            }
            println!("{}", serde_json::to_string_pretty(&results)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&persons)?);
        }
        return Ok(());
    }

    eprintln!("{} persons found.", persons.len());

    if persons.is_empty() {
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Name", "Birth", "Death"]);

    for (i, p) in persons.iter().enumerate() {
        table.add_row(vec![
            &format!("{}", i + 1),
            &p.name,
            p.birth_info.as_deref().unwrap_or("-"),
            p.death_info.as_deref().unwrap_or("-"),
        ]);

        if args.records {
            let records = db.get_person_records(p.id)?;
            for rec in &records {
                let desc = format!(
                    "    {} {} {}",
                    rec.record_type.as_deref().unwrap_or("?"),
                    rec.date.as_deref().unwrap_or(""),
                    rec.ark_url.as_deref().unwrap_or(""),
                );
                table.add_row(vec!["", &desc, "", ""]);
            }
        }
    }

    println!("{table}");
    Ok(())
}

fn run_query_ocr(args: &OcrQueryArgs, json_output: bool) -> Result<()> {
    let db = StateDb::open(&args.db)?;
    let results = db.search_ocr_text(&args.query, args.limit)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    eprintln!("{} results found.", results.len());

    if results.is_empty() {
        return Ok(());
    }

    for (i, r) in results.iter().enumerate() {
        println!(
            "{}. [{}] {} - page {} ({})",
            i + 1,
            r.backend,
            r.manifest_title.as_deref().unwrap_or("?"),
            r.canvas_index + 1,
            r.canvas_label.as_deref().unwrap_or("?"),
        );
        println!("   {}", r.snippet);
        println!();
    }

    Ok(())
}

fn run_query_manifests(args: &ManifestsQueryArgs, json_output: bool) -> Result<()> {
    let db = StateDb::open(&args.db)?;
    let results = db.search_manifests(
        args.doc_type.as_deref(),
        args.year.as_deref(),
        args.archive.as_deref(),
        args.locality.as_deref(),
    )?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    eprintln!("{} manifests found.", results.len());

    if results.is_empty() {
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Year", "Type", "Title", "Pages", "Archive"]);

    for (i, m) in results.iter().enumerate() {
        table.add_row(vec![
            &format!("{}", i + 1),
            m.year.as_deref().unwrap_or("-"),
            m.doc_type.as_deref().unwrap_or("-"),
            m.title.as_deref().unwrap_or("-"),
            &m.total_canvases.map(|c| c.to_string()).unwrap_or("-".to_string()),
            m.archive_name.as_deref().unwrap_or("-"),
        ]);
    }

    println!("{table}");
    Ok(())
}

fn run_query_stats(args: &StatsQueryArgs, json_output: bool) -> Result<()> {
    let db = StateDb::open(&args.db)?;
    let stats = db.get_extended_stats()?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }

    println!("Database Statistics");
    println!("───────────────────────────────────");
    println!("Manifests:        {}", stats.base.manifests);
    println!("Downloads:        {} ({} complete, {} failed, {} pending)",
        stats.base.total_downloads, stats.base.complete, stats.base.failed, stats.base.pending);
    println!("Sessions:         {}", stats.base.sessions);
    println!("Tags:             {}", stats.base.tags);
    println!("───────────────────────────────────");
    println!("Archives:         {}", stats.archives);
    println!("Localities:       {}", stats.localities);
    println!("Persons:          {}", stats.persons);
    println!("Search queries:   {}", stats.search_queries);
    println!("Registry results: {}", stats.registry_results);
    println!("OCR results:      {}", stats.ocr_results);

    Ok(())
}
