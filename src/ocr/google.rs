use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::config::GoogleOcrConfig;

use super::{DocumentType, OcrBackend, OcrResult};

const VISION_API_URL: &str = "https://vision.googleapis.com/v1/images:annotate";

pub struct GoogleBackend {
    client: Client,
    api_key: String,
}

impl GoogleBackend {
    pub fn new(config: &GoogleOcrConfig) -> Result<Self> {
        // Google Cloud Vision can use either API key or service account credentials.
        // We support API key via env var or credentials_path for service accounts.
        let api_key = if !config.credentials_path.is_empty() {
            // For service account, we'd need OAuth2 flow.
            // For simplicity, we support API key mode.
            anyhow::bail!(
                "Service account credentials not yet supported. Use GOOGLE_API_KEY env var instead."
            );
        } else {
            std::env::var("GOOGLE_API_KEY")
                .context("GOOGLE_API_KEY not set and no credentials_path in config")?
        };

        Ok(Self {
            client: Client::new(),
            api_key,
        })
    }
}

#[async_trait]
impl OcrBackend for GoogleBackend {
    fn name(&self) -> &str {
        "google"
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

        let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_data);

        debug!(
            "Google Vision OCR: image={}, size={}KB",
            image_path.display(),
            image_data.len() / 1024
        );

        let request_body = serde_json::json!({
            "requests": [{
                "image": {
                    "content": base64_image
                },
                "features": [{
                    "type": "DOCUMENT_TEXT_DETECTION"
                }],
                "imageContext": {
                    "languageHints": [language]
                }
            }]
        });

        let url = format!("{VISION_API_URL}?key={}", self.api_key);

        let response = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to Google Vision API")?;

        if !response.status().is_success() {
            let error_body = response.text().await.unwrap_or_default();
            anyhow::bail!("Google Vision API error: {error_body}");
        }

        let api_response: VisionResponse = response
            .json()
            .await
            .context("Failed to parse Google Vision response")?;

        let text = api_response
            .responses
            .into_iter()
            .next()
            .and_then(|r| r.full_text_annotation)
            .map(|a| a.text)
            .unwrap_or_default();

        Ok(OcrResult {
            text,
            tags: Vec::new(),
            confidence: None,
            backend: "google".to_string(),
        })
    }
}

#[derive(Deserialize)]
struct VisionResponse {
    responses: Vec<AnnotateResponse>,
}

#[derive(Deserialize)]
struct AnnotateResponse {
    #[serde(rename = "fullTextAnnotation")]
    full_text_annotation: Option<FullTextAnnotation>,
}

#[derive(Deserialize)]
struct FullTextAnnotation {
    text: String,
}
