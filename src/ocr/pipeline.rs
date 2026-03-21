use std::path::Path;

use async_trait::async_trait;

use super::{DocumentType, ExtractedTag, OcrBackend, OcrResult};

/// OCR pipeline that chains multiple backends.
///
/// The first stage performs the primary transcription.
/// Subsequent stages receive the previous text as context for refinement or entity extraction.
pub struct OcrPipeline {
    stages: Vec<Box<dyn OcrBackend>>,
}

impl OcrPipeline {
    pub fn new(stages: Vec<Box<dyn OcrBackend>>) -> anyhow::Result<Self> {
        if stages.is_empty() {
            anyhow::bail!("OCR pipeline requires at least one stage");
        }
        Ok(Self { stages })
    }
}

#[async_trait]
impl OcrBackend for OcrPipeline {
    fn name(&self) -> &str {
        "pipeline"
    }

    async fn recognize(
        &self,
        image_path: &Path,
        language: &str,
        doc_type: DocumentType,
        extract_tags: bool,
    ) -> anyhow::Result<OcrResult> {
        let mut combined_text = String::new();
        let mut all_tags: Vec<ExtractedTag> = Vec::new();
        let mut best_confidence: Option<f64> = None;
        let mut backends_used = Vec::new();

        for (i, stage) in self.stages.iter().enumerate() {
            let should_extract = if i == self.stages.len() - 1 {
                extract_tags // only extract tags on the last stage
            } else {
                false
            };

            let result = stage.recognize(image_path, language, doc_type, should_extract).await?;

            tracing::debug!(
                "Pipeline stage {} ({}): {} chars, {} tags",
                i + 1,
                stage.name(),
                result.text.len(),
                result.tags.len(),
            );

            // Use the longest/best text
            if result.text.len() > combined_text.len() {
                combined_text = result.text;
            }

            all_tags.extend(result.tags);

            if let Some(conf) = result.confidence {
                best_confidence = Some(best_confidence.map_or(conf, |c: f64| c.max(conf)));
            }

            backends_used.push(stage.name().to_string());
        }

        // Deduplicate tags by (type, value)
        all_tags.sort_by(|a, b| (&a.tag_type, &a.value).cmp(&(&b.tag_type, &b.value)));
        all_tags.dedup_by(|a, b| a.tag_type == b.tag_type && a.value == b.value);

        Ok(OcrResult {
            text: combined_text,
            tags: all_tags,
            confidence: best_confidence,
            backend: backends_used.join("+"),
        })
    }
}
