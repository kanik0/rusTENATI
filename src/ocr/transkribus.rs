use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

use crate::config::TranskribusOcrConfig;

use super::{DocumentType, OcrBackend, OcrResult};

const TRANSKRIBUS_API_URL: &str = "https://transkribus.eu/TrpServer/rest";

pub struct TranskribusBackend {
    client: Client,
    api_key: String,
    model_id: String,
}

impl TranskribusBackend {
    pub fn new(config: &TranskribusOcrConfig) -> Result<Self> {
        let api_key = if config.api_key.is_empty() {
            std::env::var("TRANSKRIBUS_API_KEY")
                .context("TRANSKRIBUS_API_KEY not set and no api_key in config")?
        } else {
            config.api_key.clone()
        };

        let model_id = if config.model_id.is_empty() {
            "italian_m1".to_string()
        } else {
            config.model_id.clone()
        };

        Ok(Self {
            client: Client::new(),
            api_key,
            model_id,
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
            "Transkribus OCR: model={}, image={}, size={}KB",
            self.model_id,
            image_path.display(),
            image_data.len() / 1024
        );

        // Step 1: Upload image and start recognition
        let upload_response = self
            .client
            .post(format!("{TRANSKRIBUS_API_URL}/recognition/upload"))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "image": base64_image,
                "modelId": self.model_id,
            }))
            .send()
            .await
            .context("Failed to upload image to Transkribus")?;

        if !upload_response.status().is_success() {
            let error_body = upload_response.text().await.unwrap_or_default();
            anyhow::bail!("Transkribus upload error: {error_body}");
        }

        let job: JobResponse = upload_response
            .json()
            .await
            .context("Failed to parse Transkribus upload response")?;

        info!("Transkribus job started: {}", job.job_id);

        // Step 2: Poll for completion
        let text = self.poll_result(&job.job_id).await?;

        Ok(OcrResult {
            text,
            tags: Vec::new(), // Transkribus doesn't extract structured tags natively
            confidence: None,
            backend: "transkribus".to_string(),
        })
    }
}

impl TranskribusBackend {
    async fn poll_result(&self, job_id: &str) -> Result<String> {
        let max_polls = 60; // 5 minutes at 5s intervals
        for i in 0..max_polls {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            let response = self
                .client
                .get(format!("{TRANSKRIBUS_API_URL}/recognition/{job_id}/result"))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await?;

            if response.status().as_u16() == 200 {
                let result: ResultResponse = response.json().await?;
                if result.status == "FINISHED" {
                    return Ok(result.text.unwrap_or_default());
                }
                if result.status == "FAILED" {
                    anyhow::bail!(
                        "Transkribus recognition failed: {}",
                        result.error.unwrap_or_default()
                    );
                }
            }

            debug!("Transkribus poll {}/{max_polls}: waiting...", i + 1);
        }

        anyhow::bail!("Transkribus recognition timed out after 5 minutes")
    }
}

#[derive(Deserialize)]
struct JobResponse {
    #[serde(alias = "jobId")]
    job_id: String,
}

#[derive(Deserialize)]
struct ResultResponse {
    status: String,
    text: Option<String>,
    error: Option<String>,
}
