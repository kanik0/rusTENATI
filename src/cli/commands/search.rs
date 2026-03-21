use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::client::antenati::AntenatiClient;
use crate::download::state::StateDb;
use crate::models::search::{NameSearchResults, RegistrySearchParams, SearchResults};

#[derive(Debug, Args)]
pub struct NameSearchArgs {
    /// Last name (required)
    #[arg(long)]
    pub surname: String,

    /// First name
    #[arg(long)]
    pub name: Option<String>,

    /// Municipality/location
    #[arg(long)]
    pub locality: Option<String>,

    /// Start year
    #[arg(long)]
    pub year_from: Option<i32>,

    /// End year
    #[arg(long)]
    pub year_to: Option<i32>,

    /// Max results
    #[arg(long, default_value = "50")]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct RegistrySearchArgs {
    /// Municipality/location
    #[arg(long)]
    pub locality: Option<String>,

    /// Archive name or slug (e.g., "archivio-di-stato-di-lucca" or "Archivio di Stato di Lucca")
    #[arg(long)]
    pub archive: Option<String>,

    /// Start year (or exact year if --year-to is not set)
    #[arg(long)]
    pub year_from: Option<i32>,

    /// End year
    #[arg(long)]
    pub year_to: Option<i32>,

    /// Document type: Nati, Morti, Matrimoni, etc.
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Results per page (10, 20, 50, 100)
    #[arg(long, default_value = "100")]
    pub page_size: u32,

    /// Page number (1-based)
    #[arg(long, default_value = "1")]
    pub page: u32,

    /// Fetch all pages at once
    #[arg(long)]
    pub all: bool,

    /// Max results (when using --all)
    #[arg(long, default_value = "1000")]
    pub limit: usize,

    /// Search local database only (offline mode)
    #[arg(long)]
    pub offline: bool,
}

pub async fn run_name_search(
    args: &NameSearchArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
    state_db: Option<&StateDb>,
) -> Result<()> {
    let results = client
        .search_names(
            &args.surname,
            args.name.as_deref(),
            args.locality.as_deref(),
            args.year_from,
            args.year_to,
            1,
            args.limit.min(100) as u32,
        )
        .await?;

    // Cache results in local database
    if let Some(db) = state_db {
        let params_json = serde_json::json!({
            "surname": args.surname,
            "name": args.name,
            "locality": args.locality,
            "year_from": args.year_from,
            "year_to": args.year_to,
        }).to_string();

        let query_id = db.insert_search_query(
            "nominative",
            &params_json,
            Some(results.total),
            Some(1),
        )?;

        for (i, result) in results.results.iter().enumerate() {
            let person_id = db.upsert_person(result)?;
            db.insert_person_search_result(query_id, person_id, i as i32)?;
        }
    }

    display_name_results(&results, json_output)
}

fn display_name_results(results: &NameSearchResults, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(results)?);
        return Ok(());
    }

    let total_display = if results.total > 0 {
        results.total
    } else {
        results.total_pages * results.page_size
    };

    eprintln!(
        "~{} results (page {}/{})",
        total_display, results.current_page, results.total_pages
    );

    if results.results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Name", "Birth", "Death", "Records"]);

    for (i, r) in results.results.iter().enumerate() {
        let birth = r.birth_info.as_deref().unwrap_or("-");
        let death = r.death_info.as_deref().unwrap_or("-");
        let records = r
            .records
            .iter()
            .filter_map(|rec| rec.date.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        let records = if records.is_empty() {
            format!("{} act(s)", r.records.len())
        } else {
            records
        };

        table.add_row(vec![
            &format!("{}", i + 1),
            &r.name,
            birth,
            death,
            &records,
        ]);
    }

    println!("{table}");
    Ok(())
}

pub async fn run_registry_search(
    args: &RegistrySearchArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
    state_db: Option<&StateDb>,
) -> Result<()> {
    // Offline mode: search local database only
    if args.offline {
        return run_registry_search_offline(args, json_output, state_db);
    }

    if args.locality.is_none() && args.archive.is_none() {
        anyhow::bail!("At least one of --locality or --archive is required");
    }

    if args.all {
        return run_registry_search_all(args, json_output, client, state_db).await;
    }

    let archive_name = resolve_archive_name(args.archive.as_deref());

    let params = RegistrySearchParams {
        locality: args.locality.as_deref(),
        archive_name: archive_name.as_deref(),
        year_from: args.year_from,
        year_to: args.year_to,
        doc_type: args.doc_type.as_deref(),
        page: args.page,
        page_size: args.page_size,
        ..Default::default()
    };

    let results = client.search_registries_params(&params).await?;

    // Cache results in local database
    cache_registry_results(state_db, args, &results)?;

    display_results(&results, json_output)
}

fn run_registry_search_offline(
    args: &RegistrySearchArgs,
    json_output: bool,
    state_db: Option<&StateDb>,
) -> Result<()> {
    let db = state_db.ok_or_else(|| anyhow::anyhow!("No database available for offline search"))?;

    let results = db.search_registry_results(
        args.doc_type.as_deref(),
        args.year_from.map(|y| y.to_string()).as_deref(),
        args.archive.as_deref(),
        args.locality.as_deref(),
    )?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    eprintln!("{} cached results found.", results.len());

    if results.is_empty() {
        println!("No cached results found. Run a search without --offline first.");
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

fn cache_registry_results(
    state_db: Option<&StateDb>,
    args: &RegistrySearchArgs,
    results: &SearchResults,
) -> Result<()> {
    let Some(db) = state_db else { return Ok(()) };

    let params_json = serde_json::json!({
        "locality": args.locality,
        "archive": args.archive,
        "year_from": args.year_from,
        "year_to": args.year_to,
        "doc_type": args.doc_type,
    }).to_string();

    let query_id = db.insert_search_query(
        "registry",
        &params_json,
        Some(results.total),
        Some(results.current_page),
    )?;

    for result in &results.results {
        db.insert_registry_result(query_id, result)?;
    }

    // Populate persistent registries catalog
    db.upsert_registries_batch(&results.results)?;

    Ok(())
}

async fn run_registry_search_all(
    args: &RegistrySearchArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
    state_db: Option<&StateDb>,
) -> Result<()> {
    let mut all_results = Vec::new();
    let mut page = 1u32;
    let mut total;
    let mut total_pages = 1u32;

    let archive_name = resolve_archive_name(args.archive.as_deref());

    loop {
        eprintln!("Fetching page {page}/{total_pages}...");

        let params = RegistrySearchParams {
            locality: args.locality.as_deref(),
            archive_name: archive_name.as_deref(),
            year_from: args.year_from,
            year_to: args.year_to,
            doc_type: args.doc_type.as_deref(),
            page,
            page_size: args.page_size,
            ..Default::default()
        };

        let results = client.search_registries_params(&params).await?;

        total = results.total;
        total_pages = results.total_pages;
        all_results.extend(results.results);

        if page >= total_pages {
            break;
        }
        if !args.all && all_results.len() >= args.limit {
            break;
        }
        page += 1;
    }

    if !args.all {
        all_results.truncate(args.limit);
    }

    let combined = SearchResults {
        total,
        current_page: 1,
        total_pages: 1,
        page_size: all_results.len() as u32,
        results: all_results,
    };

    // Cache all results
    cache_registry_results(state_db, args, &combined)?;

    display_results(&combined, json_output)
}

/// Convert a slug like "archivio-di-stato-di-lucca" to "Archivio di Stato di Lucca".
/// If the input already looks like a proper name (contains spaces), return as-is.
fn resolve_archive_name(archive: Option<&str>) -> Option<String> {
    archive.map(|a| {
        if a.contains(' ') {
            a.to_string()
        } else {
            // Convert slug to title case
            a.split('-')
                .map(|word| {
                    // Keep prepositions lowercase (di, del, della, etc.)
                    match word {
                        "di" | "del" | "della" | "delle" | "dei" | "degli" | "e" => {
                            word.to_string()
                        }
                        other => {
                            let mut chars = other.chars();
                            match chars.next() {
                                Some(c) => {
                                    c.to_uppercase().to_string() + &chars.collect::<String>()
                                }
                                None => String::new(),
                            }
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    })
}

fn display_results(results: &SearchResults, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(results)?);
        return Ok(());
    }

    let total_display = if results.total > 0 {
        results.total
    } else {
        // Estimate from pagination
        results.total_pages * results.page_size
    };

    eprintln!(
        "~{} results (page {}/{})",
        total_display, results.current_page, results.total_pages
    );

    if results.results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Year", "Type", "Location", "Archive", "ARK"]);

    for (i, r) in results.results.iter().enumerate() {
        // Extract locality from context (after " > ")
        let locality = r
            .context
            .rsplit(" > ")
            .next()
            .unwrap_or(&r.context)
            .trim();

        // Extract short ARK ID from URL
        let ark_short = r
            .ark_url
            .rsplit('/')
            .next()
            .unwrap_or(&r.ark_url);

        table.add_row(vec![
            &format!("{}", i + 1),
            &r.year,
            &r.doc_type,
            locality,
            &r.archive,
            ark_short,
        ]);
    }

    println!("{table}");
    Ok(())
}
