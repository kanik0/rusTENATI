pub mod azure;
pub mod claude_vision;
pub mod google;
pub mod transkribus;

use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::OcrConfig;

/// Result of an OCR recognition on a single image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    /// Raw transcribed text
    pub text: String,
    /// Structured tags extracted from the document
    pub tags: Vec<ExtractedTag>,
    /// Confidence score (0.0 - 1.0), if available
    pub confidence: Option<f64>,
    /// Backend that produced this result
    pub backend: String,
}

/// A structured tag extracted from an OCR result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedTag {
    /// Tag type: surname, name, date, location, event_type, role, profession
    pub tag_type: String,
    /// Extracted value
    pub value: String,
    /// Confidence (0.0 - 1.0)
    pub confidence: Option<f64>,
}

/// Document type hint for better OCR prompting.
#[derive(Debug, Clone, Copy)]
pub enum DocumentType {
    Birth,
    Death,
    Marriage,
    Unknown,
}

impl DocumentType {
    pub fn as_italian(&self) -> &str {
        match self {
            Self::Birth => "nascita",
            Self::Death => "morte",
            Self::Marriage => "matrimonio",
            Self::Unknown => "stato civile",
        }
    }
}

/// Trait for OCR backends.
#[async_trait]
pub trait OcrBackend: Send + Sync {
    /// Backend name (e.g., "claude", "transkribus")
    fn name(&self) -> &str;

    /// Recognize text in a single image.
    async fn recognize(
        &self,
        image_path: &Path,
        language: &str,
        doc_type: DocumentType,
        extract_tags: bool,
    ) -> anyhow::Result<OcrResult>;
}

/// Create an OCR backend from configuration.
pub fn create_backend(config: &OcrConfig, backend_name: Option<&str>) -> anyhow::Result<Box<dyn OcrBackend>> {
    let name = backend_name.unwrap_or(&config.default_backend);

    match name {
        "claude" => {
            let api_key_available = !config.claude.api_key.is_empty()
                || std::env::var("ANTHROPIC_API_KEY").is_ok();
            if !api_key_available {
                anyhow::bail!(
                    "Claude API key not configured. Set ANTHROPIC_API_KEY env var or api_key in config.toml [ocr.claude]."
                );
            }
            Ok(Box::new(claude_vision::ClaudeVisionBackend::new(
                &config.claude,
            )?))
        }
        "transkribus" => {
            Ok(Box::new(transkribus::TranskribusBackend::new(
                &config.transkribus,
            )?))
        }
        "azure" => {
            Ok(Box::new(azure::AzureBackend::new(&config.azure)?))
        }
        "google" => {
            Ok(Box::new(google::GoogleBackend::new(&config.google)?))
        }
        other => {
            anyhow::bail!("Unknown OCR backend: '{other}'. Available: claude, transkribus, azure, google")
        }
    }
}
