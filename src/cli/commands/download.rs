use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use futures_util::future;
use futures_util::stream::{self, StreamExt};

use crate::client::antenati::AntenatiClient;
use crate::client::rate_limiter;
use crate::download::engine::{self, DownloadConfig, DownloadSummary, PageRange};
use crate::download::state::StateDb;
use crate::models::search::RegistrySearchParams;
use crate::output;

#[derive(Debug, Args)]
pub struct DownloadArgs {
    /// Manifest URL, archive ID, ARK identifier, or "search:" prefix for batch
    pub source: Option<String>,

    /// Parallel downloads
    #[arg(short, long, default_value = "8")]
    pub jobs: usize,

    /// Image format: jpg, png
    #[arg(long, default_value = "jpg")]
    pub format: String,

    /// Delay between requests in ms
    #[arg(long, default_value = "500")]
    pub delay: u64,

    /// Resume a previous download
    #[arg(long)]
    pub resume: bool,

    /// Show what would be downloaded without downloading
    #[arg(long)]
    pub dry_run: bool,

    /// Page range: "1-50" or "10,20,30-40"
    #[arg(long)]
    pub pages: Option<String>,

    /// Skip files already on disk
    #[arg(long)]
    pub skip_existing: bool,

    // --- Batch download from search ---
    /// Download all registries matching a search (requires --locality)
    #[arg(long)]
    pub search: bool,

    /// Locality for batch search download
    #[arg(long)]
    pub locality: Option<String>,

    /// Archive name or slug for batch download (e.g., "archivio-di-stato-di-lucca")
    #[arg(long)]
    pub archive: Option<String>,

    /// Start year for batch search
    #[arg(long)]
    pub year_from: Option<i32>,

    /// End year for batch search
    #[arg(long)]
    pub year_to: Option<i32>,

    /// Document type filter for batch search
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Filter results by locality name (case-insensitive substring match)
    #[arg(long)]
    pub filter: Option<String>,

    /// Max registries to download in batch mode
    #[arg(long, default_value = "100")]
    pub max_registries: usize,

    /// Fetch all registries (ignores --max-registries limit)
    #[arg(long)]
    pub all: bool,

    // --- Noah mode: dump EVERYTHING ---
    /// Noah mode: dump ALL archives, ALL registries, ALL images from the entire portal
    #[arg(long)]
    pub noah: bool,

    /// Max archives to process in Noah mode (0 = all)
    #[arg(long, default_value = "0")]
    pub max_archives: usize,

    // --- Performance tuning ---
    /// Explicit rate limit in requests per second (overrides --delay for rate limiting)
    #[arg(long)]
    pub rps: Option<u32>,

    /// Max idle connections per host in the HTTP pool
    #[arg(long)]
    pub connections: Option<usize>,

    /// Retry previously failed downloads
    #[arg(long)]
    pub retry_failed: bool,

    /// Enable adaptive concurrency (AIMD: auto-adjusts parallelism based on server response)
    #[arg(long)]
    pub adaptive: bool,
}

pub async fn run(
    args: &DownloadArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    if args.noah {
        return run_noah_mode(args, json_output, client).await;
    }

    if args.search {
        return run_batch_download(args, json_output, client).await;
    }

    let source = args
        .source
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Source required (manifest URL, ARK, or use --search or --noah)"))?;

    run_single_download(source, args, json_output, client).await
}

/// Compute the effective rate limiter: --rps takes priority, otherwise derive from --delay.
fn effective_rps(args: &DownloadArgs) -> u32 {
    if let Some(rps) = args.rps {
        rps.max(1)
    } else if args.delay > 0 {
        (1000 / args.delay).max(1) as u32
    } else {
        10
    }
}

async fn run_single_download(
    source: &str,
    args: &DownloadArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    let manifest_url = client.resolve_manifest_url(source).await?;
    let manifest = client.get_manifest(&manifest_url).await?;
    let output_dir = output::build_output_dir(&output::base_dir(), &manifest);

    let state_db_path = output::db_path();
    let state_db = StateDb::open(&state_db_path)?;

    // Populate registry catalog from manifest metadata
    let ark_url = if source.contains("ark:") {
        Some(source)
    } else {
        None
    };
    state_db.upsert_registry_from_manifest(&manifest, ark_url)?;

    // Reset failed downloads for retry
    if args.retry_failed {
        let reset = state_db.reset_failed_to_pending(&manifest.id)?;
        if reset > 0 {
            eprintln!("Reset {reset} failed downloads for retry");
        }
    }

    let limiter = rate_limiter::create_rate_limiter(effective_rps(args));

    let page_range = args
        .pages
        .as_deref()
        .map(PageRange::parse)
        .transpose()?;

    let config = DownloadConfig {
        concurrency: args.jobs,
        image_format: args.format.clone(),
        delay_ms: args.delay,
        dry_run: args.dry_run,
        skip_existing: args.skip_existing,
        page_range,
        resume: args.resume,
        adaptive: args.adaptive,
    };

    let summary = engine::download_manifest(
        client,
        limiter,
        &state_db,
        &manifest,
        &output_dir,
        &config,
        ark_url.map(|s| s as &str),
    )
    .await?;

    print_summary(&summary, &output_dir, json_output);
    Ok(())
}

async fn run_batch_download(
    args: &DownloadArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    if args.locality.is_none() && args.archive.is_none() {
        anyhow::bail!("At least one of --locality or --archive is required for batch search download");
    }

    let archive_name = args.archive.as_deref().map(|a| {
        if a.contains(' ') {
            a.to_string()
        } else {
            // Convert slug to title case
            a.split('-')
                .map(|word| match word {
                    "di" | "del" | "della" | "delle" | "dei" | "degli" | "e" => word.to_string(),
                    other => {
                        let mut chars = other.chars();
                        match chars.next() {
                            Some(c) => c.to_uppercase().to_string() + &chars.collect::<String>(),
                            None => String::new(),
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    });

    let filter_desc = match (&args.locality, &archive_name) {
        (Some(loc), Some(arch)) => format!("locality '{loc}' in archive '{arch}'"),
        (Some(loc), None) => format!("locality '{loc}'"),
        (None, Some(arch)) => format!("archive '{arch}'"),
        (None, None) => unreachable!(),
    };
    eprintln!("Searching registries for {filter_desc}...");

    let mut all_results = Vec::new();
    let page_size = 100u32;
    let filter_lower = args.filter.as_ref().map(|f| f.to_lowercase());

    // Fetch first page to discover total_pages
    let first_params = RegistrySearchParams {
        locality: args.locality.as_deref(),
        archive_name: archive_name.as_deref(),
        year_from: args.year_from,
        year_to: args.year_to,
        doc_type: args.doc_type.as_deref(),
        page: 1,
        page_size,
        ..Default::default()
    };
    let first_page = client.search_registries_params(&first_params).await?;
    let total_pages = first_page.total_pages;
    all_results.extend(first_page.results);

    // Fetch remaining pages in parallel
    if total_pages > 1 && (args.all || all_results.len() < args.max_registries) {
        let remaining: Vec<u32> = (2..=total_pages).collect();
        let page_futures = remaining.into_iter().map(|p| {
            let client = client.clone();
            let locality = args.locality.clone();
            let archive_name = archive_name.clone();
            let year_from = args.year_from;
            let year_to = args.year_to;
            let doc_type = args.doc_type.clone();
            async move {
                let params = RegistrySearchParams {
                    locality: locality.as_deref(),
                    archive_name: archive_name.as_deref(),
                    year_from,
                    year_to,
                    doc_type: doc_type.as_deref(),
                    page: p,
                    page_size,
                    ..Default::default()
                };
                client.search_registries_params(&params).await
            }
        });
        let page_results = futures_util::future::join_all(page_futures).await;
        for result in page_results {
            match result {
                Ok(page_data) => all_results.extend(page_data.results),
                Err(e) => eprintln!("Warning: failed to fetch search page: {e}"),
            }
        }
    }

    // Apply filter
    if let Some(ref filter) = filter_lower {
        all_results.retain(|r| {
            let loc = r.context.rsplit(" > ").next().unwrap_or(&r.context).trim();
            loc.to_lowercase().contains(filter)
        });
    }

    if !args.all {
        all_results.truncate(args.max_registries);
    }

    let total_registries = all_results.len();
    eprintln!("Found {total_registries} registries to download.");

    if total_registries == 0 {
        eprintln!("No registries found.");
        return Ok(());
    }

    // Persist found registries to catalog
    let state_db_path = output::db_path();
    let state_db = StateDb::open(&state_db_path)?;
    state_db.upsert_registries_batch(&all_results)?;

    // Dry run: just list
    if args.dry_run {
        println!("Dry run: would download {} registries:", total_registries);
        for (i, r) in all_results.iter().enumerate() {
            let loc = r.context.rsplit(" > ").next().unwrap_or(&r.context).trim();
            println!("  {:3}. {} - {} - {} ({})", i + 1, r.year, r.doc_type, loc, r.ark_url);
        }
        return Ok(());
    }

    let limiter = rate_limiter::create_rate_limiter(effective_rps(args));

    let page_range = args
        .pages
        .as_deref()
        .map(PageRange::parse)
        .transpose()?;

    let config = DownloadConfig {
        concurrency: args.jobs,
        image_format: args.format.clone(),
        delay_ms: args.delay,
        dry_run: false,
        skip_existing: args.skip_existing,
        page_range,
        resume: args.resume,
        adaptive: args.adaptive,
    };

    let mut total_summary = DownloadSummary::default();
    let mut failed_registries = Vec::new();

    for (i, result) in all_results.iter().enumerate() {
        let loc = result
            .context
            .rsplit(" > ")
            .next()
            .unwrap_or(&result.context)
            .trim();
        eprintln!(
            "\n[{}/{}] {} - {} - {}",
            i + 1,
            total_registries,
            result.year,
            result.doc_type,
            loc,
        );

        // Resolve ARK to manifest
        let manifest_url = match client.resolve_manifest_url(&result.ark_url).await {
            Ok(url) => url,
            Err(e) => {
                eprintln!("  Error resolving manifest: {e}");
                failed_registries.push(result.ark_url.clone());
                continue;
            }
        };

        let manifest = match client.get_manifest(&manifest_url).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("  Error fetching manifest: {e}");
                failed_registries.push(result.ark_url.clone());
                continue;
            }
        };

        let output_dir = output::build_output_dir(&output::base_dir(), &manifest);

        let summary = match engine::download_manifest(
            client.clone(),
            limiter.clone(),
            &state_db,
            &manifest,
            &output_dir,
            &config,
            Some(&result.ark_url),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  Error downloading: {e}");
                failed_registries.push(result.ark_url.clone());
                continue;
            }
        };

        eprintln!("  {summary}");
        total_summary.total += summary.total;
        total_summary.downloaded += summary.downloaded;
        total_summary.skipped += summary.skipped;
        total_summary.failed += summary.failed;
        total_summary.cancelled += summary.cancelled;
    }

    eprintln!();
    eprintln!("Batch download complete!");
    eprintln!(
        "Registries: {} total, {} failed",
        total_registries,
        failed_registries.len()
    );

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "registries_total": total_registries,
                "registries_failed": failed_registries.len(),
                "images_total": total_summary.total,
                "images_downloaded": total_summary.downloaded,
                "images_skipped": total_summary.skipped,
                "images_failed": total_summary.failed,
                "failed_arks": failed_registries,
                "output_dir": output::base_dir().display().to_string(),
            })
        );
    } else {
        println!("Images: {total_summary}");
        println!("Output: {}", output::base_dir().display());
        if !failed_registries.is_empty() {
            eprintln!("\nFailed registries:");
            for ark in &failed_registries {
                eprintln!("  {ark}");
            }
        }
    }

    Ok(())
}

/// Noah mode: dump ALL images from ALL archives on the entire portal.
async fn run_noah_mode(
    args: &DownloadArgs,
    json_output: bool,
    client: Arc<AntenatiClient>,
) -> Result<()> {
    eprintln!("=== NOAH MODE ===");
    eprintln!("Listing all archives on Portale Antenati...");

    let mut archives = client.list_archives().await?;

    if archives.is_empty() {
        anyhow::bail!("No archives found on the portal");
    }

    // Optionally limit the number of archives
    if args.max_archives > 0 && archives.len() > args.max_archives {
        archives.truncate(args.max_archives);
    }

    let total_archives = archives.len();
    eprintln!("Found {total_archives} archives to process.");

    let state_db_path = output::db_path();
    let state_db = StateDb::open(&state_db_path)?;

    // Persist archives to catalog
    for a in &archives {
        state_db.upsert_archive(&a.name, &a.slug, Some(&a.url))?;
    }

    let limiter = rate_limiter::create_rate_limiter(effective_rps(args));

    let page_range = args
        .pages
        .as_deref()
        .map(PageRange::parse)
        .transpose()?;

    let config = DownloadConfig {
        concurrency: args.jobs,
        image_format: args.format.clone(),
        delay_ms: args.delay,
        dry_run: false,
        skip_existing: args.skip_existing,
        page_range,
        resume: args.resume,
        adaptive: args.adaptive,
    };

    let mut grand_total = DownloadSummary::default();
    let mut total_registries_processed = 0usize;
    let mut total_registries_failed = 0usize;
    let mut failed_archives: Vec<String> = Vec::new();

    for (archive_idx, archive) in archives.iter().enumerate() {
        eprintln!(
            "\n{}\n[Archive {}/{}] {}",
            "=".repeat(60),
            archive_idx + 1,
            total_archives,
            archive.name,
        );

        // Convert slug to archive name for Solr query
        let archive_name = archive.name.clone();

        // Fetch all registries for this archive (parallel pagination)
        let mut all_results = Vec::new();
        let page_size = 100u32;

        let first_params = RegistrySearchParams {
            archive_name: Some(&archive_name),
            year_from: args.year_from,
            year_to: args.year_to,
            doc_type: args.doc_type.as_deref(),
            page: 1,
            page_size,
            ..Default::default()
        };

        match client.search_registries_params(&first_params).await {
            Ok(first_page) => {
                let total_pages = first_page.total_pages;
                eprintln!("  {} registries found", first_page.total);
                all_results.extend(first_page.results);

                // Fetch remaining pages in parallel
                if total_pages > 1 {
                    let remaining: Vec<u32> = (2..=total_pages).collect();
                    let page_futures = remaining.into_iter().map(|p| {
                        let client = client.clone();
                        let archive_name = archive_name.clone();
                        let year_from = args.year_from;
                        let year_to = args.year_to;
                        let doc_type = args.doc_type.clone();
                        async move {
                            let params = RegistrySearchParams {
                                archive_name: Some(&archive_name),
                                year_from,
                                year_to,
                                doc_type: doc_type.as_deref(),
                                page: p,
                                page_size,
                                ..Default::default()
                            };
                            client.search_registries_params(&params).await
                        }
                    });
                    let page_results = future::join_all(page_futures).await;
                    for result in page_results {
                        match result {
                            Ok(page_data) => all_results.extend(page_data.results),
                            Err(e) => eprintln!("  Warning: failed to fetch search page: {e}"),
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("  Error searching archive: {e}");
                failed_archives.push(archive.name.clone());
                continue;
            }
        }

        if !args.all && args.max_registries > 0 {
            all_results.truncate(args.max_registries);
        }

        if all_results.is_empty() {
            eprintln!("  No registries found, skipping.");
            continue;
        }

        // Persist found registries to catalog
        state_db.upsert_registries_batch(&all_results)?;

        let archive_registries = all_results.len();

        // Dry run: list registries and skip download
        if args.dry_run {
            println!(
                "Dry run: {} registries in {}:",
                archive_registries, archive.name
            );
            for (reg_idx, result) in all_results.iter().enumerate() {
                let loc = result
                    .context
                    .rsplit(" > ")
                    .next()
                    .unwrap_or(&result.context)
                    .trim();
                println!(
                    "  {:3}. {} - {} - {} [{}]",
                    reg_idx + 1,
                    result.year,
                    result.doc_type,
                    loc,
                    result.ark_url,
                );
            }
            total_registries_processed += archive_registries;
            continue;
        }

        // Download registries with manifest-level concurrency (4 at a time)
        // Uses buffer_unordered to avoid spawning tasks (StateDb is !Sync)
        const MANIFEST_CONCURRENCY: usize = 4;

        let results: Vec<Option<DownloadSummary>> = stream::iter(
            all_results.iter().enumerate()
        )
        .map(|(reg_idx, result)| {
            let client = client.clone();
            let limiter = limiter.clone();
            let config = &config;
            let state_db = &state_db;
            async move {
                let loc = result.context.rsplit(" > ").next().unwrap_or(&result.context).trim();
                eprintln!(
                    "  [{}/{}] {} - {} - {}",
                    reg_idx + 1, archive_registries, result.year, result.doc_type, loc,
                );

                let manifest_url = match client.resolve_manifest_url(&result.ark_url).await {
                    Ok(url) => url,
                    Err(e) => {
                        eprintln!("    Error resolving manifest: {e}");
                        return None;
                    }
                };

                let manifest = match client.get_manifest(&manifest_url).await {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("    Error fetching manifest: {e}");
                        return None;
                    }
                };

                let output_dir = output::build_output_dir(&output::base_dir(), &manifest);

                let summary = match engine::download_manifest(
                    client, limiter, state_db, &manifest, &output_dir, config, Some(&result.ark_url),
                ).await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("    Error downloading: {e}");
                        return None;
                    }
                };

                eprintln!("    {summary}");
                Some(summary)
            }
        })
        .buffer_unordered(MANIFEST_CONCURRENCY)
        .collect()
        .await;

        for result in results {
            match result {
                Some(summary) => {
                    grand_total.total += summary.total;
                    grand_total.downloaded += summary.downloaded;
                    grand_total.skipped += summary.skipped;
                    grand_total.failed += summary.failed;
                    grand_total.cancelled += summary.cancelled;
                    total_registries_processed += 1;
                }
                None => {
                    total_registries_failed += 1;
                }
            }
        }
    }

    eprintln!();
    eprintln!("=== NOAH MODE COMPLETE ===");
    eprintln!(
        "Archives: {} processed, {} failed",
        total_archives - failed_archives.len(),
        failed_archives.len()
    );
    eprintln!(
        "Registries: {} processed, {} failed",
        total_registries_processed, total_registries_failed
    );

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "mode": "noah",
                "archives_total": total_archives,
                "archives_failed": failed_archives.len(),
                "registries_processed": total_registries_processed,
                "registries_failed": total_registries_failed,
                "images_total": grand_total.total,
                "images_downloaded": grand_total.downloaded,
                "images_skipped": grand_total.skipped,
                "images_failed": grand_total.failed,
                "failed_archives": failed_archives,
                "output_dir": output::base_dir().display().to_string(),
            })
        );
    } else {
        println!("Images: {grand_total}");
        println!("Output: {}", output::base_dir().display());
        if !failed_archives.is_empty() {
            eprintln!("\nFailed archives:");
            for name in &failed_archives {
                eprintln!("  {name}");
            }
        }
    }

    Ok(())
}

fn print_summary(summary: &DownloadSummary, output_dir: &std::path::Path, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "total": summary.total,
                "downloaded": summary.downloaded,
                "skipped": summary.skipped,
                "failed": summary.failed,
                "output_dir": output_dir.display().to_string(),
            })
        );
    } else {
        println!();
        println!("Download complete: {summary}");
        println!("Output: {}", output_dir.display());
    }
}
