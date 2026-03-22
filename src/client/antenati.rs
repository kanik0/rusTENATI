use std::sync::Arc;

use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use tracing::{debug, error, warn};

use crate::client::iiif;
use crate::config::HttpConfig;
use crate::error::RustenatiError;
use crate::models::manifest::IiifManifest;
use crate::models::search::{
    ArchiveInfo, LinkedRecord, NameResult, NameSearchResults, RegistryResult,
    RegistrySearchParams, SearchResults,
};

const REFERER: &str = "https://antenati.cultura.gov.it/";
const BASE_URL: &str = "https://antenati.cultura.gov.it";
const DAM_URL: &str = "https://dam-antenati.cultura.gov.it";

/// Main client for interacting with the Portale Antenati API.
pub struct AntenatiClient {
    http: Client,
    base_url: String,
    dam_url: String,
    api_max_retries: u32,
    api_initial_backoff_ms: u64,
}

impl AntenatiClient {
    pub fn new(config: &HttpConfig) -> anyhow::Result<Arc<Self>> {
        let mut headers = HeaderMap::new();
        headers.insert(header::REFERER, HeaderValue::from_static(REFERER));

        let mut builder = Client::builder()
            .default_headers(headers)
            .user_agent(&config.user_agent)
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(std::time::Duration::from_secs(config.pool_idle_timeout_secs))
            .cookie_store(true)
            .gzip(true)
            .brotli(true);

        if let Some(keepalive) = config.tcp_keepalive_secs {
            builder = builder.tcp_keepalive(std::time::Duration::from_secs(keepalive));
        }

        let http = builder.build()?;

        debug!("HTTP client configured (rustls-tls, HTTP/2 ALPN enabled)");

        Ok(Arc::new(Self {
            http,
            base_url: BASE_URL.to_string(),
            dam_url: DAM_URL.to_string(),
            api_max_retries: config.api_max_retries,
            api_initial_backoff_ms: config.api_initial_backoff_ms,
        }))
    }

    /// Parse the Retry-After header value (integer seconds) from a response.
    fn parse_retry_after(response: &reqwest::Response) -> Option<u64> {
        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
    }

    /// Perform an HTTP GET with automatic retry on 5xx and 429 errors.
    ///
    /// Returns the successful response (2xx) or an error after retries are exhausted.
    /// WAF-related statuses (202, 405) are returned as-is without retry.
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response, RustenatiError> {
        let mut backoff_ms = self.api_initial_backoff_ms;
        let mut last_status = None;
        let mut last_retry_after = None;

        for attempt in 1..=self.api_max_retries {
            let response = self.http.get(url).send().await?;
            let status = response.status();

            if status.is_success() {
                // WAF challenges come as 202 — let the caller handle them
                return Ok(response);
            }

            // WAF-like statuses: don't retry, let caller inspect
            if status.as_u16() == 202 || status.as_u16() == 405 {
                return Ok(response);
            }

            let retry_after = Self::parse_retry_after(&response);
            let status_code = status.as_u16();

            // Only retry on 5xx and 429
            let is_retryable = status.is_server_error() || status_code == 429;
            if !is_retryable {
                return Err(RustenatiError::UnexpectedStatus {
                    status: status_code,
                    url: url.to_string(),
                });
            }

            last_status = Some(status_code);
            last_retry_after = retry_after;

            if attempt == self.api_max_retries {
                break;
            }

            // Calculate wait time
            let wait_ms = if let Some(ra) = retry_after {
                // Respect Retry-After if it's larger than our backoff
                (ra * 1000).max(backoff_ms)
            } else if status_code == 429 {
                // Default 429 wait: use backoff * 2
                backoff_ms * 2
            } else {
                backoff_ms
            };

            // Add jitter: ±25%
            let jitter = (wait_ms as f64 * 0.25 * (fastrand::f64() * 2.0 - 1.0)) as i64;
            let actual_wait = (wait_ms as i64 + jitter).max(100) as u64;

            warn!(
                url = %url,
                status = status_code,
                attempt = attempt,
                max_retries = self.api_max_retries,
                wait_ms = actual_wait,
                retry_after = ?retry_after,
                "Server error, retrying"
            );

            tokio::time::sleep(std::time::Duration::from_millis(actual_wait)).await;
            backoff_ms = (backoff_ms * 2).min(30_000);
        }

        // All retries exhausted
        let status = last_status.unwrap_or(503);
        error!(url = %url, status, attempts = self.api_max_retries, "Request failed after all retries");

        if status == 429 {
            Err(RustenatiError::RateLimited {
                retry_after_secs: last_retry_after.unwrap_or(60),
            })
        } else {
            Err(RustenatiError::ServerUnavailable {
                status,
                url: url.to_string(),
                retry_after_secs: last_retry_after,
            })
        }
    }

    /// Fetch and parse an IIIF manifest from a manifest URL.
    pub async fn get_manifest(&self, manifest_url: &str) -> Result<IiifManifest, RustenatiError> {
        debug!("Fetching manifest: {manifest_url}");

        let response = self.get_with_retry(manifest_url).await?;

        // Check for WAF challenge (HTTP 202 or 405 with challenge body)
        let status_code = response.status().as_u16();
        if status_code == 202 || status_code == 405 {
            let body = response.text().await.unwrap_or_default();
            if body.contains("aws-waf") || body.contains("challenge") {
                return Err(RustenatiError::WafChallenge {
                    challenge_url: manifest_url.to_string(),
                    body,
                });
            }
            // Not a WAF challenge — try to parse as JSON anyway
            let json: Value = serde_json::from_str(&body)
                .map_err(|e| RustenatiError::ManifestParse(e.to_string()))?;
            return iiif::parse_manifest(&json);
        }

        let json: Value = response.json().await?;
        iiif::parse_manifest(&json)
    }

    /// Conditionally fetch a manifest, using ETag/Last-Modified for cache validation.
    /// Returns `Ok(None)` if the manifest has not changed (304 Not Modified).
    /// Returns `Ok(Some((manifest, etag, last_modified)))` if changed or no cache headers.
    pub async fn get_manifest_conditional(
        &self,
        manifest_url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<Option<(IiifManifest, Option<String>, Option<String>)>, RustenatiError> {
        debug!("Conditional fetch: {manifest_url}");

        let mut request = self.http.get(manifest_url);
        if let Some(etag_val) = etag {
            request = request.header("If-None-Match", etag_val);
        }
        if let Some(lm) = last_modified {
            request = request.header("If-Modified-Since", lm);
        }

        let response = request.send().await?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            debug!("Manifest not modified: {manifest_url}");
            return Ok(None);
        }

        if !response.status().is_success() {
            // Fall back to regular fetch with retry for errors
            let manifest = self.get_manifest(manifest_url).await?;
            return Ok(Some((manifest, None, None)));
        }

        // Extract cache headers before consuming the response
        let resp_etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let resp_last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let json: serde_json::Value = response.json().await?;
        let manifest = iiif::parse_manifest(&json)?;

        Ok(Some((manifest, resp_etag, resp_last_modified)))
    }

    /// Fetch the gallery HTML page and extract the manifest URL from it.
    pub async fn find_manifest_url(&self, gallery_url: &str) -> Result<String, RustenatiError> {
        debug!("Fetching gallery page: {gallery_url}");

        let response = self.get_with_retry(gallery_url).await?;
        let html = response.text().await?;

        // Look for manifest URL pattern in the HTML (Mirador viewer configuration)
        let manifest_pattern = format!(
            "{}/antenati/containers/",
            self.dam_url
        );
        if let Some(start) = html.find(&manifest_pattern) {
            // Find the end of the URL (quote or whitespace)
            let url_start = start;
            let rest = &html[url_start..];
            let end = rest
                .find(|c: char| c == '"' || c == '\'' || c == ',' || c.is_whitespace())
                .unwrap_or(rest.len());
            let manifest_url = &rest[..end];
            debug!("Found manifest URL: {manifest_url}");
            return Ok(manifest_url.to_string());
        }

        warn!("Could not find manifest URL in gallery page");
        Err(RustenatiError::ManifestParse(
            "Could not find manifest URL in gallery page HTML".into(),
        ))
    }

    /// Resolve a source string to a manifest URL.
    /// Accepts: manifest URL, gallery URL, or ARK identifier.
    pub async fn resolve_manifest_url(&self, source: &str) -> Result<String, RustenatiError> {
        // Already a manifest URL
        if source.contains("/manifest") || source.contains("/manifest.json") {
            return Ok(source.to_string());
        }

        // DAM container URL without /manifest
        if source.contains("dam-antenati") && source.contains("/containers/") {
            let url = if source.ends_with('/') {
                format!("{source}manifest")
            } else {
                format!("{source}/manifest")
            };
            return Ok(url);
        }

        // ARK identifier or gallery URL
        if source.contains("ark:/") || source.starts_with("https://antenati.cultura.gov.it/") {
            let gallery_url = if source.starts_with("http") {
                source.to_string()
            } else {
                format!("{}/{source}", self.base_url)
            };
            return self.find_manifest_url(&gallery_url).await;
        }

        // Assume it's a container UUID
        let url = format!("{}/antenati/containers/{source}/manifest", self.dam_url);
        Ok(url)
    }

    /// List all archives (Archivi di Stato) from the portal.
    pub async fn list_archives(&self) -> Result<Vec<ArchiveInfo>, RustenatiError> {
        let url = format!("{}/esplora-gli-archivi/", self.base_url);
        debug!("Fetching archives list: {url}");

        let response = self.get_with_retry(&url).await?;
        let html = response.text().await?;
        Self::parse_archives_html(&html)
    }

    /// Parse the archives listing page HTML.
    fn parse_archives_html(html: &str) -> Result<Vec<ArchiveInfo>, RustenatiError> {
        let document = Html::parse_document(html);

        // Archives are listed as links in the page, typically under
        // <a href="/archivio/archivio-di-stato-di-{slug}/">Name</a>
        let a_sel = Selector::parse("a[href*='/archivio/']").unwrap();

        let mut archives = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for a in document.select(&a_sel) {
            let href = a.value().attr("href").unwrap_or("");
            let name = a.text().collect::<String>().trim().to_string();

            // Skip empty names, navigation links, etc.
            if name.is_empty() || !href.contains("/archivio/") {
                continue;
            }

            // Extract slug from URL: /archivio/{slug}/
            let slug = href
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();

            if slug.is_empty() || slug == "archivio" || !seen.insert(slug.clone()) {
                continue;
            }

            let url = if href.starts_with('/') {
                format!("{}{}", BASE_URL, href)
            } else if href.starts_with("http") {
                href.to_string()
            } else {
                continue;
            };

            archives.push(ArchiveInfo { name, slug, url });
        }

        Ok(archives)
    }

    /// Search registries with structured parameters (supports archive filtering).
    pub async fn search_registries_params(
        &self,
        params: &RegistrySearchParams<'_>,
    ) -> Result<SearchResults, RustenatiError> {
        let mut url = format!("{}/search-registry/?", self.base_url);
        let mut first = true;

        let mut add_param = |key: &str, value: &str| {
            if !first {
                url.push('&');
            }
            url.push_str(&format!("{key}={value}"));
            first = false;
        };

        if let Some(loc) = params.locality {
            add_param("localita", loc);
        }

        if let Some(id) = params.archive_id {
            add_param("archivio", id);
        } else if let Some(name) = params.archive_name {
            add_param("archivio", name);
        }

        if let Some(from) = params.year_from {
            if let Some(to) = params.year_to {
                if from == to {
                    add_param("anno", &from.to_string());
                } else {
                    add_param("anno_da", &from.to_string());
                    add_param("anno_a", &to.to_string());
                }
            } else {
                add_param("anno", &from.to_string());
            }
        }

        if let Some(dt) = params.doc_type {
            add_param("tipologia", dt);
        }

        if params.page > 1 {
            add_param("s_page", &params.page.to_string());
        }

        if params.page_size != 10 {
            add_param("s_size", &params.page_size.to_string());
        }

        if let Some(s) = params.sort {
            add_param("s_sort", s);
        }

        add_param("lang", "it");

        debug!("Search URL: {url}");

        let response = self.get_with_retry(&url).await?;
        let html = response.text().await?;
        Self::parse_registry_search_html(&html, params.page, params.page_size)
    }

    /// Search registries with the given parameters.
    pub async fn search_registries(
        &self,
        locality: &str,
        year_from: Option<i32>,
        year_to: Option<i32>,
        doc_type: Option<&str>,
        page: u32,
        page_size: u32,
        sort: Option<&str>,
    ) -> Result<SearchResults, RustenatiError> {
        self.search_registries_params(&RegistrySearchParams {
            locality: Some(locality),
            year_from,
            year_to,
            doc_type,
            page,
            page_size,
            sort,
            ..Default::default()
        })
        .await
    }

    /// Parse the HTML search results page.
    fn parse_registry_search_html(
        html: &str,
        page: u32,
        page_size: u32,
    ) -> Result<SearchResults, RustenatiError> {
        let document = Html::parse_document(html);

        // Extract total count: <span>NNN</span> followed by "risultati"
        let total = Self::extract_total_count(html);

        // Parse pagination: "Pagina X di Y"
        let (current_page, total_pages) = Self::extract_pagination(html).unwrap_or((page, 1));

        // Parse result items: li.search-item
        let li_sel = Selector::parse("li.search-item").unwrap();
        let h3_sel = Selector::parse("h3 a").unwrap();
        let p_sel = Selector::parse("p").unwrap();
        let a_sel = Selector::parse("a").unwrap();

        let mut results = Vec::new();

        for li in document.select(&li_sel) {
            // Extract ARK URL from the h3 > a
            let ark_url = li
                .select(&h3_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .unwrap_or("")
                .to_string();

            // Clean up: remove ?lang=it suffix and make absolute
            let ark_url = ark_url
                .split('?')
                .next()
                .unwrap_or(&ark_url)
                .to_string();
            let ark_url = if ark_url.starts_with('/') {
                format!("{}{}", BASE_URL, ark_url)
            } else {
                ark_url
            };

            // Extract year from h3 text ("Registro: 1810")
            let year = li
                .select(&h3_sel)
                .next()
                .map(|a| {
                    a.text()
                        .collect::<String>()
                        .trim()
                        .strip_prefix("Registro: ")
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default();

            // Extract paragraphs: doc_type, signature, context, archive
            let paragraphs: Vec<String> = li
                .select(&p_sel)
                .map(|p| p.text().collect::<String>().trim().to_string())
                .collect();

            let doc_type = paragraphs.first().cloned().unwrap_or_default();
            let signature = paragraphs
                .get(1)
                .and_then(|s| s.strip_prefix("Segnatura attuale:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let context = paragraphs.get(2).cloned().unwrap_or_default();

            // Archive name and URL from the last <p> containing <a>
            let (archive, archive_url) = paragraphs
                .get(3)
                .map(|p| {
                    let name = p
                        .strip_prefix("Conservato da:")
                        .unwrap_or(p)
                        .trim()
                        .to_string();
                    (name, None)
                })
                .unwrap_or_default();

            // Try to get archive URL from the last p > a
            let archive_url = li
                .select(&p_sel)
                .last()
                .and_then(|p| p.select(&a_sel).next())
                .and_then(|a| a.value().attr("href"))
                .map(|u| u.to_string())
                .or(archive_url);

            if !ark_url.is_empty() {
                results.push(RegistryResult {
                    ark_url,
                    year,
                    doc_type,
                    signature,
                    context,
                    archive,
                    archive_url,
                });
            }
        }

        Ok(SearchResults {
            total,
            current_page,
            total_pages,
            page_size,
            results,
        })
    }

    /// Extract total result count from HTML.
    fn extract_total_count(html: &str) -> u32 {
        // Try multiple patterns for the total count
        // Pattern 1: <span>NNN</span> followed by "risultati"
        for keyword in ["risultati", "risultato", "results", "result"] {
            if let Some(pos) = html.find(keyword) {
                let before = &html[..pos];
                // Look for the last number before "risultati"
                if let Some(span_end) = before.rfind("</span>") {
                    let before_span = &before[..span_end];
                    if let Some(span_start) = before_span.rfind('>') {
                        let num_str = before_span[span_start + 1..].trim();
                        if let Ok(n) = num_str.parse::<u32>() {
                            return n;
                        }
                    }
                }
                // Pattern 2: just a number before the keyword (no span)
                let trimmed = before.trim();
                let last_word = trimmed.rsplit(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
                if let Ok(n) = last_word.parse::<u32>() {
                    return n;
                }
            }
        }

        // Pattern 3: look for data-total or similar attributes
        for attr in ["data-total", "data-count", "data-num-found"] {
            if let Some(pos) = html.find(attr) {
                let rest = &html[pos + attr.len()..];
                if let Some(eq) = rest.find('=') {
                    let after_eq = rest[eq + 1..].trim_start_matches(|c: char| c == '"' || c == '\'');
                    let num: String = after_eq.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = num.parse::<u32>() {
                        return n;
                    }
                }
            }
        }

        0
    }

    /// Extract pagination info from HTML: "Pagina X di Y"
    fn extract_pagination(html: &str) -> Option<(u32, u32)> {
        if let Some(pos) = html.find("Pagina ") {
            let rest = &html[pos + 7..];
            let parts: Vec<&str> = rest.splitn(4, ' ').collect();
            if parts.len() >= 3 && parts[1] == "di" {
                let current = parts[0].parse().ok()?;
                // total_pages might have trailing HTML
                let total_str: String = parts[2].chars().take_while(|c| c.is_ascii_digit()).collect();
                let total = total_str.parse().ok()?;
                return Some((current, total));
            }
        }
        None
    }

    /// Search by name (nominative search).
    pub async fn search_names(
        &self,
        surname: &str,
        name: Option<&str>,
        locality: Option<&str>,
        year_from: Option<i32>,
        year_to: Option<i32>,
        page: u32,
        page_size: u32,
    ) -> Result<NameSearchResults, RustenatiError> {
        let mut url = format!(
            "{}/search-nominative/?cognome={}",
            self.base_url, surname
        );

        if let Some(n) = name {
            url.push_str(&format!("&nome={n}"));
        }
        if let Some(loc) = locality {
            url.push_str(&format!("&luogo_nascita={loc}"));
        }
        if let Some(from) = year_from {
            url.push_str(&format!("&anno_nascita_da={from}"));
        }
        if let Some(to) = year_to {
            url.push_str(&format!("&anno_nascita_a={to}"));
        }
        if page > 1 {
            url.push_str(&format!("&s_page={page}"));
        }
        if page_size != 10 {
            url.push_str(&format!("&s_size={page_size}"));
        }
        url.push_str("&lang=it");

        debug!("Name search URL: {url}");

        let response = self.get_with_retry(&url).await?;
        let html = response.text().await?;
        Self::parse_name_search_html(&html, page, page_size)
    }

    /// Parse the HTML name search results page.
    fn parse_name_search_html(
        html: &str,
        page: u32,
        page_size: u32,
    ) -> Result<NameSearchResults, RustenatiError> {
        let document = Html::parse_document(html);

        let total = Self::extract_total_count(html);
        let (current_page, total_pages) = Self::extract_pagination(html).unwrap_or((page, 1));

        // Parse result items: div.search-item inside ul.no-appearance > li
        let item_sel = Selector::parse("div.search-item").unwrap();
        let h3_sel = Selector::parse("h3").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let detail_sel = Selector::parse(".nominative-detail").unwrap();
        let records_sel = Selector::parse(".nominative-records").unwrap();

        let mut results = Vec::new();

        for item in document.select(&item_sel) {
            // Extract name and detail URL from h3 > a
            let (name, detail_url) = item
                .select(&h3_sel)
                .next()
                .and_then(|h3| h3.select(&a_sel).next())
                .map(|a| {
                    let name = a.text().collect::<String>().trim().to_string();
                    let href = a.value().attr("href").unwrap_or("").to_string();
                    let href = href.split('?').next().unwrap_or(&href).to_string();
                    let href = if href.starts_with('/') {
                        format!("{}{}", BASE_URL, href)
                    } else {
                        href
                    };
                    (name, href)
                })
                .unwrap_or_default();

            if name.is_empty() {
                continue;
            }

            // Extract birth/death info from detail
            let (birth_info, death_info) = if let Some(detail) = item.select(&detail_sel).next() {
                let text = detail.text().collect::<String>();
                let birth = text
                    .find("Nascita:")
                    .map(|pos| {
                        text[pos + 8..]
                            .lines()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .take(2)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty());
                let death = text
                    .find("Morte:")
                    .map(|pos| {
                        text[pos + 6..]
                            .lines()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .take(2)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty());
                (birth, death)
            } else {
                (None, None)
            };

            // Extract linked records
            let mut records = Vec::new();
            if let Some(recs) = item.select(&records_sel).next() {
                // Each linked record is typically: record_type + date + optional link
                for a in recs.select(&a_sel) {
                    let href = a.value().attr("href").unwrap_or("");
                    let text = a.text().collect::<String>().trim().to_string();

                    if href.contains("ark:") {
                        // This is an ARK link to a document
                        let ark = href.split('?').next().unwrap_or(href);
                        let ark = if ark.starts_with('/') {
                            format!("{}{}", BASE_URL, ark)
                        } else {
                            ark.to_string()
                        };
                        records.push(LinkedRecord {
                            record_type: String::new(),
                            date: Some(text),
                            ark_url: Some(ark),
                        });
                    } else if !text.is_empty() && !href.contains("detail-nominative") {
                        records.push(LinkedRecord {
                            record_type: text,
                            date: None,
                            ark_url: None,
                        });
                    }
                }

                // Merge consecutive record_type + date entries
                // Pattern: "Atto di nascita" link + "Atto senza data" ARK link
                // The ARK link text often contains the date info
            }

            results.push(NameResult {
                name,
                detail_url,
                birth_info,
                death_info,
                records,
            });
        }

        Ok(NameSearchResults {
            total,
            current_page,
            total_pages,
            page_size,
            results,
        })
    }

    /// Fetch the autocomplete suggestions for a locality.
    pub async fn suggest_locality(&self, query: &str) -> Result<Vec<String>, RustenatiError> {
        let url = format!(
            "{}/suggest/?campo=localita&localita={}&tipologia=",
            self.base_url, query
        );

        debug!("Suggest URL: {url}");
        let response = self.get_with_retry(&url).await?;

        // The suggest endpoint returns a JSON array of strings
        let suggestions: Vec<String> = response.json().await.unwrap_or_default();
        Ok(suggestions)
    }

    /// Get the underlying HTTP client.
    pub fn http(&self) -> &Client {
        &self.http
    }
}
