use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::config::AzureOcrConfig;

use super::{DocumentType, OcrBackend, OcrResult};

pub struct AzureBackend {
    client: Client,
    endpoint: String,
    api_key: String,
}

impl AzureBackend {
    pub fn new(config: &AzureOcrConfig) -> Result<Self> {
        let api_key = if config.api_key.is_empty() {
            std::env::var("AZURE_OCR_API_KEY")
                .context("AZURE_OCR_API_KEY not set and no api_key in config")?
        } else {
            config.api_key.clone()
        };

        let endpoint = if config.endpoint.is_empty() {
            std::env::var("AZURE_OCR_ENDPOINT")
                .context("AZURE_OCR_ENDPOINT not set and no endpoint in config")?
        } else {
            config.endpoint.clone()
        };

        Ok(Self {
            client: Client::new(),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            api_key,
        })
    }
}

#[async_trait]
impl OcrBackend for AzureBackend {
    fn name(&self) -> &str {
        "azure"
    }

    async fn recognize(
        &self,
        image_path: &Path,
        language: &str,
        _doc_type: DocumentType,
        _extract_tags: bool,
    ) -> Result<OcrResult> {
        let image_data = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("Failed to read image: {}", image_path.display()))?;

        let content_type = match image_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
        {
            "png" => "image/png",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "tiff" | "tif" => "image/tiff",
            _ => "image/jpeg",
        };

        debug!(
            "Azure OCR: image={}, size={}KB, language={}",
            image_path.display(),
            image_data.len() / 1024,
            language
        );

        // Step 1: Submit image for analysis
        let analyze_url = format!(
            "{}/formrecognizer/documentModels/prebuilt-read:analyze?api-version=2023-07-31&locale={}",
            self.endpoint, language
        );

        let response = self
            .client
            .post(&analyze_url)
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
            .header("Content-Type", content_type)
            .body(image_data)
            .send()
            .await
            .context("Failed to submit image to Azure")?;

        if !response.status().is_success() {
            let error_body = response.text().await.unwrap_or_default();
            anyhow::bail!("Azure API error: {error_body}");
        }

        // Get operation location for polling
        let operation_url = response
            .headers()
            .get("operation-location")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Azure response missing operation-location header"))?;

        // Step 2: Poll for result
        let text = self.poll_result(&operation_url).await?;

        Ok(OcrResult {
            text,
            tags: Vec::new(),
            confidence: None,
            backend: "azure".to_string(),
        })
    }
}

impl AzureBackend {
    async fn poll_result(&self, operation_url: &str) -> Result<String> {
        let max_polls = 60;
        for i in 0..max_polls {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let response = self
                .client
                .get(operation_url)
                .header("Ocp-Apim-Subscription-Key", &self.api_key)
                .send()
                .await?;

            let result: AnalyzeResult = response
                .json()
                .await
                .context("Failed to parse Azure poll response")?;

            match result.status.as_str() {
                "succeeded" => {
                    let text = result
                        .analyze_result
                        .map(|ar| ar.content)
                        .unwrap_or_default();
                    return Ok(text);
                }
                "failed" => {
                    anyhow::bail!("Azure analysis failed");
                }
                _ => {
                    debug!("Azure poll {}/{max_polls}: status={}", i + 1, result.status);
                }
            }
        }

        anyhow::bail!("Azure analysis timed out after 2 minutes")
    }
}

#[derive(Deserialize)]
struct AnalyzeResult {
    status: String,
    #[serde(rename = "analyzeResult")]
    analyze_result: Option<AnalyzeContent>,
}

#[derive(Deserialize)]
struct AnalyzeContent {
    content: String,
}
