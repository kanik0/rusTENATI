use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::client::antenati::AntenatiClient;
use crate::client::rate_limiter::Limiter;
use crate::client::waf;
use crate::download::progress;
use crate::download::state::StateDb;
use crate::models::manifest::{Canvas, IiifManifest};
use crate::output;

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Configuration for the download engine.
pub struct DownloadConfig {
    pub concurrency: usize,
    pub image_format: String,
    pub delay_ms: u64,
    pub dry_run: bool,
    pub skip_existing: bool,
    pub page_range: Option<PageRange>,
    pub resume: bool,
}

/// Parsed page range (e.g., "1-50" or "10,20,30-40").
#[derive(Debug, Clone)]
pub struct PageRange {
    ranges: Vec<(usize, usize)>,
}

impl PageRange {
    /// Parse a page range string like "1-50" or "10,20,30-40".
    pub fn parse(input: &str) -> Result<Self> {
        let mut ranges = Vec::new();
        for part in input.split(',') {
            let part = part.trim();
            if let Some((start, end)) = part.split_once('-') {
                let start: usize = start.trim().parse().context("Invalid page range start")?;
                let end: usize = end.trim().parse().context("Invalid page range end")?;
                ranges.push((start, end));
            } else {
                let page: usize = part.parse().context("Invalid page number")?;
                ranges.push((page, page));
            }
        }
        Ok(Self { ranges })
    }

    /// Check if a 1-based page number is included in this range.
    pub fn includes(&self, page: usize) -> bool {
        self.ranges.iter().any(|(start, end)| page >= *start && page <= *end)
    }
}

/// Run the download pipeline for a manifest.
pub async fn download_manifest(
    client: Arc<AntenatiClient>,
    rate_limiter: Limiter,
    state_db: &StateDb,
    manifest: &IiifManifest,
    output_dir: &Path,
    config: &DownloadConfig,
    ark_url: Option<&str>,
) -> Result<DownloadSummary> {
    // Filter canvases by page range
    let canvases: Vec<(usize, &Canvas)> = manifest
        .canvases
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            config
                .page_range
                .as_ref()
                .map_or(true, |range| range.includes(i + 1))
        })
        .collect();

    let total = canvases.len();

    if total == 0 {
        warn!("No canvases to download");
        return Ok(DownloadSummary::default());
    }

    info!("Downloading {} images from '{}'", total, manifest.title());

    // Dry run: just list what would be downloaded
    if config.dry_run {
        println!("Dry run: would download {} images", total);
        for (i, canvas) in &canvases {
            let filename = output::image_filename(*i, &canvas.label, &config.image_format);
            let url = canvas.full_image_url(&config.image_format);
            println!("  {} → {}", url, filename);
        }
        return Ok(DownloadSummary {
            total,
            ..Default::default()
        });
    }

    // Create output directories
    output::ensure_output_dirs(output_dir)?;
    output::write_manifest_json(output_dir, manifest)?;
    output::write_metadata_json(output_dir, manifest, &chrono::Utc::now().to_rfc3339())?;

    // Register manifest with full metadata and downloads in state DB
    state_db.store_manifest_from_iiif(manifest, ark_url)?;

    for (i, canvas) in &canvases {
        let url = canvas.full_image_url(&config.image_format);
        state_db.insert_download_full(
            &manifest.id, &canvas.id, *i, &url,
            Some(&canvas.label), Some(canvas.width), Some(canvas.height),
        )?;
    }

    // Resume: filter out already completed downloads
    let canvases: Vec<(usize, &Canvas)> = if config.resume {
        let mut filtered = Vec::new();
        for (i, canvas) in canvases {
            if !state_db.is_downloaded(&manifest.id, &canvas.id)? {
                filtered.push((i, canvas));
            }
        }
        let skipped = total - filtered.len();
        if skipped > 0 {
            info!("Resuming: skipping {skipped} already completed downloads");
        }
        filtered
    } else {
        canvases
    };

    let remaining = canvases.len();

    // Set up graceful shutdown
    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\nShutting down gracefully... (press Ctrl+C again to force quit)");
            cancel_clone.cancel();
        }
    });

    // Create progress bar
    let multi = progress::create_multi_progress();
    let main_bar = multi.add(progress::create_main_bar(remaining as u64));

    // Download with semaphore-limited concurrency
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut handles = Vec::with_capacity(remaining);

    let images_dir = output_dir.join("images");

    for (i, canvas) in canvases {
        // Check cancellation before spawning new tasks
        if cancel_token.is_cancelled() {
            break;
        }

        let permit = semaphore.clone().acquire_owned().await?;
        let client = client.clone();
        let rate_limiter = rate_limiter.clone();
        let manifest_id = manifest.id.clone();
        let canvas_id = canvas.id.clone();
        let canvas_label = canvas.label.clone();
        let image_url = canvas.full_image_url(&config.image_format);
        let filename = output::image_filename(i, &canvas.label, &config.image_format);
        let filepath = images_dir.join(&filename);
        let main_bar = main_bar.clone();
        let skip_existing = config.skip_existing;
        let delay_ms = config.delay_ms;
        let cancel = cancel_token.clone();

        let handle = tokio::spawn(async move {
            let result = download_with_retry(
                &client,
                &rate_limiter,
                &image_url,
                &filepath,
                &canvas_label,
                skip_existing,
                delay_ms,
                &cancel,
            )
            .await;

            main_bar.inc(1);
            drop(permit);

            match result {
                Ok(DownloadOutcome::Downloaded(checksum)) => {
                    DownloadResult {
                        manifest_id,
                        canvas_id,
                        local_path: filepath.to_string_lossy().to_string(),
                        sha256: checksum,
                        skipped: false,
                        error: None,
                    }
                }
                Ok(DownloadOutcome::Skipped(checksum)) => {
                    DownloadResult {
                        manifest_id,
                        canvas_id,
                        local_path: filepath.to_string_lossy().to_string(),
                        sha256: checksum,
                        skipped: true,
                        error: None,
                    }
                }
                Ok(DownloadOutcome::Cancelled) => {
                    DownloadResult {
                        manifest_id,
                        canvas_id,
                        local_path: String::new(),
                        sha256: String::new(),
                        skipped: false,
                        error: Some("cancelled".to_string()),
                    }
                }
                Err(e) => {
                    error!("Failed to download {filename}: {e}");
                    DownloadResult {
                        manifest_id,
                        canvas_id,
                        local_path: String::new(),
                        sha256: String::new(),
                        skipped: false,
                        error: Some(e.to_string()),
                    }
                }
            }
        });

        handles.push(handle);
    }

    // Collect results
    let mut summary = DownloadSummary {
        total,
        ..Default::default()
    };

    for handle in handles {
        let result = handle.await?;
        if let Some(ref err) = result.error {
            if err == "cancelled" {
                summary.cancelled += 1;
            } else {
                state_db.mark_failed(&result.manifest_id, &result.canvas_id, err)?;
                summary.failed += 1;
            }
        } else if result.skipped {
            summary.skipped += 1;
        } else {
            state_db.mark_complete(
                &result.manifest_id,
                &result.canvas_id,
                &result.local_path,
                &result.sha256,
            )?;
            summary.downloaded += 1;
        }
    }

    main_bar.finish_with_message(if cancel_token.is_cancelled() {
        "interrupted - progress saved!"
    } else {
        "done!"
    });

    if cancel_token.is_cancelled() {
        eprintln!(
            "Download interrupted. {} images saved. Use --resume to continue later.",
            summary.downloaded
        );
    }

    Ok(summary)
}

enum DownloadOutcome {
    Downloaded(String),
    Skipped(String),
    Cancelled,
}

async fn download_with_retry(
    client: &AntenatiClient,
    rate_limiter: &Limiter,
    url: &str,
    filepath: &Path,
    label: &str,
    skip_existing: bool,
    delay_ms: u64,
    cancel: &CancellationToken,
) -> Result<DownloadOutcome> {
    // Skip if file already exists
    if skip_existing && filepath.exists() {
        debug!("Skipping existing file: {}", filepath.display());
        let data = tokio::fs::read(filepath).await?;
        let checksum = hex::encode(Sha256::digest(&data));
        return Ok(DownloadOutcome::Skipped(checksum));
    }

    let mut last_error = None;
    let mut backoff_ms = INITIAL_BACKOFF_MS;

    for attempt in 1..=MAX_RETRIES {
        // Check cancellation
        if cancel.is_cancelled() {
            return Ok(DownloadOutcome::Cancelled);
        }

        // Rate limit
        rate_limiter.until_ready().await;

        // Optional delay
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        if attempt > 1 {
            debug!("Retry {attempt}/{MAX_RETRIES} for {label}");
        } else {
            debug!("Downloading: {label} → {}", filepath.display());
        }

        match attempt_download(client, url, filepath).await {
            Ok(checksum) => return Ok(DownloadOutcome::Downloaded(checksum)),
            Err(e) => {
                let is_retryable = is_retryable_error(&e);
                let status_code = extract_status_code(&e);

                if !is_retryable || attempt == MAX_RETRIES {
                    last_error = Some(e);
                    break;
                }

                // Handle specific status codes
                let wait = match status_code {
                    Some(429) => {
                        // Rate limited - use longer backoff
                        warn!("Rate limited (429) on {label}, backing off {backoff_ms}ms");
                        backoff_ms * 2
                    }
                    Some(403) => {
                        // Possible WAF challenge
                        warn!("HTTP 403 on {label}, may be WAF challenge");
                        backoff_ms
                    }
                    Some(500..=599) => {
                        warn!("Server error ({}) on {label}, retrying", status_code.unwrap());
                        backoff_ms
                    }
                    _ => {
                        warn!("Error on {label}: {e}, retrying in {backoff_ms}ms");
                        backoff_ms
                    }
                };

                // Add jitter: ±25%
                let jitter = (wait as f64 * 0.25 * (rand_jitter() * 2.0 - 1.0)) as u64;
                let actual_wait = (wait as i64 + jitter as i64).max(100) as u64;

                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(actual_wait)) => {}
                    _ = cancel.cancelled() => {
                        return Ok(DownloadOutcome::Cancelled);
                    }
                }

                last_error = Some(e);
                backoff_ms = (backoff_ms * 2).min(30_000); // Cap at 30s
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Download failed after {MAX_RETRIES} retries")))
}

async fn attempt_download(
    client: &AntenatiClient,
    url: &str,
    filepath: &Path,
) -> Result<String> {
    let response = client.http().get(url).send().await?;
    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();

        // Check for WAF challenge
        if waf::is_waf_challenge(status.as_u16(), &body) {
            info!("Attempting to solve WAF challenge");
            let _ = waf::try_solve_challenge(client.http(), url, &body).await;
            anyhow::bail!("WAF challenge on {url} (HTTP {status})");
        }

        anyhow::bail!("HTTP {} for {}", status.as_u16(), url);
    }

    let bytes = response.bytes().await?;

    if bytes.is_empty() {
        anyhow::bail!("Empty response for {url}");
    }

    // Compute SHA256
    let checksum = hex::encode(Sha256::digest(&bytes));

    // Write to disk
    tokio::fs::write(filepath, &bytes).await?;

    Ok(checksum)
}

fn is_retryable_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string();

    // HTTP status-based
    if msg.contains("HTTP 429") || msg.contains("HTTP 5") || msg.contains("WAF challenge") {
        return true;
    }

    // Network errors
    if msg.contains("connection")
        || msg.contains("timeout")
        || msg.contains("reset")
        || msg.contains("broken pipe")
    {
        return true;
    }

    // Check for reqwest errors
    if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_timeout()
            || reqwest_err.is_connect()
            || reqwest_err.is_request();
    }

    false
}

fn extract_status_code(e: &anyhow::Error) -> Option<u16> {
    let msg = e.to_string();
    // Match "HTTP 429", "HTTP 503", etc.
    if let Some(pos) = msg.find("HTTP ") {
        let after = &msg[pos + 5..];
        if let Some(code_str) = after.split_whitespace().next() {
            return code_str.parse().ok();
        }
    }
    if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
        return reqwest_err.status().map(|s| s.as_u16());
    }
    None
}

/// Simple pseudo-random jitter in [0.0, 1.0) using time-based seed.
fn rand_jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}

struct DownloadResult {
    manifest_id: String,
    canvas_id: String,
    local_path: String,
    sha256: String,
    skipped: bool,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub struct DownloadSummary {
    pub total: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl std::fmt::Display for DownloadSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Total: {}, Downloaded: {}, Skipped: {}, Failed: {}",
            self.total, self.downloaded, self.skipped, self.failed
        )?;
        if self.cancelled > 0 {
            write!(f, ", Cancelled: {}", self.cancelled)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_range_parse() {
        let range = PageRange::parse("1-50").unwrap();
        assert!(range.includes(1));
        assert!(range.includes(50));
        assert!(!range.includes(51));

        let range = PageRange::parse("10,20,30-40").unwrap();
        assert!(range.includes(10));
        assert!(range.includes(20));
        assert!(range.includes(35));
        assert!(!range.includes(11));
        assert!(!range.includes(41));
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable_error(&anyhow::anyhow!("HTTP 429 for url")));
        assert!(is_retryable_error(&anyhow::anyhow!("HTTP 503 for url")));
        assert!(is_retryable_error(&anyhow::anyhow!("WAF challenge on url")));
        assert!(is_retryable_error(&anyhow::anyhow!("connection reset")));
        assert!(!is_retryable_error(&anyhow::anyhow!("HTTP 404 not found")));
        assert!(!is_retryable_error(&anyhow::anyhow!("invalid JSON")));
    }

    #[test]
    fn test_extract_status_code() {
        assert_eq!(extract_status_code(&anyhow::anyhow!("HTTP 429 for url")), Some(429));
        assert_eq!(extract_status_code(&anyhow::anyhow!("HTTP 503 for url")), Some(503));
        assert_eq!(extract_status_code(&anyhow::anyhow!("some other error")), None);
    }
}
