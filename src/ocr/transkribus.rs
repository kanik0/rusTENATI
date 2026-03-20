use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::TranskribusOcrConfig;

use super::{DocumentType, OcrBackend, OcrResult};

const METAGRAPHO_API_URL: &str = "https://transkribus.eu/processing/v1";

pub struct TranskribusBackend {
    client: Client,
    access_token: String,
    htr_id: i64,
}

impl TranskribusBackend {
    pub fn new(config: &TranskribusOcrConfig) -> Result<Self> {
        let access_token = if config.access_token.is_empty() {
            std::env::var("TRANSKRIBUS_ACCESS_TOKEN")
                .context("TRANSKRIBUS_ACCESS_TOKEN not set and no access_token in config")?
        } else {
            config.access_token.clone()
        };

        let htr_id = if config.htr_id != 0 {
            config.htr_id
        } else if !config.model_id.is_empty() {
            config.model_id.parse::<i64>()
                .context("model_id must be a numeric HTR model ID (see Transkribus model list)")?
        } else {
            anyhow::bail!(
                "Transkribus requires htr_id (numeric model ID). \
                 Find your model ID at https://www.transkribus.org/models"
            )
        };

        Ok(Self {
            client: Client::new(),
            access_token,
            htr_id,
        })
    }
}

#[async_trait]
impl OcrBackend for TranskribusBackend {
    fn name(&self) -> &str {
        "transkribus"
    }

    async fn recognize(
        &self,
        image_path: &Path,
        _language: &str,
        _doc_type: DocumentType,
        _extract_tags: bool,
    ) -> Result<OcrResult> {
        let image_data = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("Failed to read image: {}", image_path.display()))?;

        let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_data);

        debug!(
            "Transkribus OCR: htr_id={}, image={}, size={}KB",
            self.htr_id,
            image_path.display(),
            image_data.len() / 1024
        );

        // Step 1: Submit image for recognition via metagrapho API
        let submit_response = self
            .client
            .post(format!("{METAGRAPHO_API_URL}/processes"))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&serde_json::json!({
                "config": {
                    "textRecognition": {
                        "htrId": self.htr_id
                    }
                },
                "image": {
                    "base64": base64_image
                }
            }))
            .send()
            .await
            .context("Failed to submit image to Transkribus")?;

        if !submit_response.status().is_success() {
            let status = submit_response.status();
            let error_body = submit_response.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                anyhow::bail!(
                    "Transkribus authentication failed (401). \
                     Your access token may be expired — obtain a new one from \
                     https://account.readcoop.eu/auth/realms/readcoop/protocol/openid-connect/token"
                );
            }
            anyhow::bail!("Transkribus submit error ({status}): {error_body}");
        }

        let process: ProcessResponse = submit_response
            .json()
            .await
            .context("Failed to parse Transkribus submit response")?;

        info!("Transkribus process started: {}", process.process_id);

        // Step 2: Poll for completion
        let text = self.poll_result(process.process_id).await?;

        Ok(OcrResult {
            text,
            tags: Vec::new(),
            confidence: None,
            backend: "transkribus".to_string(),
        })
    }
}

impl TranskribusBackend {
    async fn poll_result(&self, process_id: i64) -> Result<String> {
        let max_polls = 60; // 5 minutes at 5s intervals
        for i in 0..max_polls {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            let response = self
                .client
                .get(format!("{METAGRAPHO_API_URL}/processes/{process_id}"))
                .header("Authorization", format!("Bearer {}", self.access_token))
                .send()
                .await?;

            if !response.status().is_success() {
                warn!("Transkribus poll returned status {}", response.status());
                continue;
            }

            let result: ProcessStatusResponse = response
                .json()
                .await
                .context("Failed to parse Transkribus status response")?;

            match result.status.as_str() {
                "FINISHED" => {
                    // Extract text from the process result
                    return self.fetch_text_result(process_id).await;
                }
                "FAILED" => {
                    anyhow::bail!(
                        "Transkribus recognition failed: {}",
                        result.description.unwrap_or_default()
                    );
                }
                status => {
                    debug!("Transkribus poll {}/{max_polls}: status={status}", i + 1);
                }
            }
        }

        anyhow::bail!("Transkribus recognition timed out after 5 minutes")
    }

    async fn fetch_text_result(&self, process_id: i64) -> Result<String> {
        // Fetch ALTO XML and extract text, or fall back to PAGE XML
        let response = self
            .client
            .get(format!("{METAGRAPHO_API_URL}/processes/{process_id}/alto"))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await
            .context("Failed to fetch Transkribus ALTO result")?;

        if response.status().is_success() {
            let alto_xml = response.text().await?;
            return Ok(extract_text_from_alto(&alto_xml));
        }

        // Fallback: try PAGE XML
        let response = self
            .client
            .get(format!("{METAGRAPHO_API_URL}/processes/{process_id}/page"))
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await
            .context("Failed to fetch Transkribus PAGE result")?;

        if response.status().is_success() {
            let page_xml = response.text().await?;
            return Ok(extract_text_from_page(&page_xml));
        }

        anyhow::bail!("Failed to retrieve text from Transkribus process {process_id}")
    }
}

/// Extract plain text from ALTO XML by finding all <String CONTENT="..."> elements.
fn extract_text_from_alto(xml: &str) -> String {
    let mut lines = Vec::new();
    let mut current_line = Vec::new();

    for segment in xml.split('<') {
        let segment = segment.trim();
        if segment.starts_with("String ") || segment.starts_with("String\t") {
            if let Some(content) = extract_xml_attr(segment, "CONTENT") {
                current_line.push(content);
            }
        } else if segment.starts_with("TextLine") && !segment.contains('/') {
            if !current_line.is_empty() {
                lines.push(current_line.join(" "));
                current_line.clear();
            }
        } else if segment.starts_with("/TextBlock") || segment.starts_with("/PrintSpace") {
            if !current_line.is_empty() {
                lines.push(current_line.join(" "));
                current_line.clear();
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line.join(" "));
    }

    lines.join("\n")
}

/// Extract plain text from PAGE XML by finding all <Unicode> elements.
fn extract_text_from_page(xml: &str) -> String {
    let mut lines = Vec::new();
    let mut in_unicode = false;
    let mut current_text = String::new();

    for segment in xml.split('<') {
        if in_unicode {
            if let Some(text) = segment.split('>').next() {
                if text.starts_with("/Unicode") {
                    lines.push(current_text.clone());
                    current_text.clear();
                    in_unicode = false;
                }
            }
        }
        if segment.starts_with("Unicode") {
            in_unicode = true;
            if let Some(rest) = segment.split('>').nth(1) {
                current_text.push_str(rest);
            }
        }
    }

    lines.join("\n")
}

fn extract_xml_attr<'a>(tag: &'a str, attr: &str) -> Option<String> {
    let pattern = format!("{attr}=\"");
    let start = tag.find(&pattern)? + pattern.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(html_unescape(&rest[..end]))
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[derive(Deserialize)]
struct ProcessResponse {
    #[serde(alias = "processId")]
    process_id: i64,
}

#[derive(Deserialize)]
struct ProcessStatusResponse {
    status: String,
    description: Option<String>,
}
