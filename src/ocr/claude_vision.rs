use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::ClaudeOcrConfig;

use super::{DocumentType, ExtractedTag, OcrBackend, OcrResult};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

pub struct ClaudeVisionBackend {
    client: Client,
    api_key: String,
    model: String,
}

impl ClaudeVisionBackend {
    pub fn new(config: &ClaudeOcrConfig) -> Result<Self> {
        let api_key = if config.api_key.is_empty() {
            std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY not set and no api_key in config")?
        } else {
            config.api_key.clone()
        };

        let model = if config.model.is_empty() {
            "claude-sonnet-4-6".to_string()
        } else {
            config.model.clone()
        };

        Ok(Self {
            client: Client::new(),
            api_key,
            model,
        })
    }
}

#[async_trait]
impl OcrBackend for ClaudeVisionBackend {
    fn name(&self) -> &str {
        "claude"
    }

    async fn recognize(
        &self,
        image_path: &Path,
        language: &str,
        doc_type: DocumentType,
        extract_tags: bool,
    ) -> Result<OcrResult> {
        let image_data = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("Failed to read image: {}", image_path.display()))?;

        let media_type = match image_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg")
        {
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => "image/jpeg",
        };

        let base64_image = base64::engine::general_purpose::STANDARD.encode(&image_data);

        let prompt = build_prompt(language, doc_type, extract_tags);

        debug!(
            "OCR request: model={}, image={}, size={}KB",
            self.model,
            image_path.display(),
            image_data.len() / 1024
        );

        let request_body = ApiRequest {
            model: &self.model,
            max_tokens: 4096,
            messages: vec![Message {
                role: "user",
                content: vec![
                    ContentBlock::Image {
                        r#type: "image",
                        source: ImageSource {
                            r#type: "base64",
                            media_type,
                            data: &base64_image,
                        },
                    },
                    ContentBlock::Text {
                        r#type: "text",
                        text: &prompt,
                    },
                ],
            }],
        };

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send OCR request to Anthropic API")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error (HTTP {status}): {error_body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("Failed to parse Anthropic API response")?;

        let raw_text = api_response
            .content
            .iter()
            .filter_map(|block| {
                if let ResponseBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let (text, tags) = if extract_tags {
            parse_structured_response(&raw_text)
        } else {
            (raw_text, Vec::new())
        };

        Ok(OcrResult {
            text,
            tags,
            confidence: None,
            backend: "claude".to_string(),
        })
    }
}

fn build_prompt(language: &str, doc_type: DocumentType, extract_tags: bool) -> String {
    let lang_instruction = match language {
        "it" => "Il documento è scritto in italiano.",
        "la" => "Il documento è scritto in latino.",
        _ => "Identifica la lingua del documento e trascrivi di conseguenza.",
    };

    let doc_type_str = doc_type.as_italian();

    let base = format!(
        "Sei un esperto paleografo italiano specializzato in documenti storici.\n\
         {lang_instruction}\n\
         Questo è un atto di {doc_type_str} dal registro di stato civile.\n\n\
         Trascrivi fedelmente tutto il testo manoscritto visibile nell'immagine.\n\
         - Mantieni la struttura originale (paragrafi, a capo)\n\
         - Indica con [?] le parole incerte o illeggibili\n\
         - Non aggiungere interpretazioni o commenti\n\
         - Trascrivi numeri e date come appaiono nel documento"
    );

    if extract_tags {
        format!(
            "{base}\n\n\
             Dopo la trascrizione, aggiungi un blocco JSON delimitato da ```json e ``` con i seguenti campi estratti:\n\
             {{\n  \
               \"cognomi\": [\"...\"],\n  \
               \"nomi\": [\"...\"],\n  \
               \"date\": [\"YYYY-MM-DD o come appaiono\"],\n  \
               \"localita\": [\"...\"],\n  \
               \"tipo_evento\": \"nascita|morte|matrimonio|altro\",\n  \
               \"ruoli\": [{{\"nome\": \"...\", \"ruolo\": \"padre|madre|testimone|ufficiale|...\"}}],\n  \
               \"professioni\": [\"...\"]\n\
             }}\n\
             Includi solo i campi per cui trovi dati nel documento."
        )
    } else {
        base
    }
}

/// Parse a response that may contain both text and a JSON block with tags.
fn parse_structured_response(raw: &str) -> (String, Vec<ExtractedTag>) {
    // Look for ```json ... ``` block
    let (text, json_str) = if let Some(json_start) = raw.find("```json") {
        let before = raw[..json_start].trim();
        let after_marker = &raw[json_start + 7..];
        let json_end = after_marker.find("```").unwrap_or(after_marker.len());
        let json = after_marker[..json_end].trim();
        // Any text after the closing ``` is also part of the transcription
        let remaining = if json_end + 3 < after_marker.len() {
            after_marker[json_end + 3..].trim()
        } else {
            ""
        };
        let text = if remaining.is_empty() {
            before.to_string()
        } else {
            format!("{before}\n{remaining}")
        };
        (text, Some(json))
    } else {
        (raw.to_string(), None)
    };

    let tags = json_str
        .and_then(|json| {
            serde_json::from_str::<TagsJson>(json)
                .map_err(|e| {
                    warn!("Failed to parse structured tags JSON: {e}");
                    e
                })
                .ok()
        })
        .map(|parsed| parsed.into_tags())
        .unwrap_or_default();

    (text, tags)
}

// --- API request/response types ---

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: Vec<ContentBlock<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ContentBlock<'a> {
    Image {
        r#type: &'a str,
        source: ImageSource<'a>,
    },
    Text {
        r#type: &'a str,
        text: &'a str,
    },
}

#[derive(Serialize)]
struct ImageSource<'a> {
    r#type: &'a str,
    media_type: &'a str,
    data: &'a str,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ResponseBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

// --- Structured tags JSON ---

#[derive(Deserialize, Default)]
struct TagsJson {
    #[serde(default)]
    cognomi: Vec<String>,
    #[serde(default)]
    nomi: Vec<String>,
    #[serde(default)]
    date: Vec<String>,
    #[serde(default)]
    localita: Vec<String>,
    #[serde(default)]
    tipo_evento: Option<String>,
    #[serde(default)]
    ruoli: Vec<RoleJson>,
    #[serde(default)]
    professioni: Vec<String>,
}

#[derive(Deserialize)]
struct RoleJson {
    nome: String,
    ruolo: String,
}

impl TagsJson {
    fn into_tags(self) -> Vec<ExtractedTag> {
        let mut tags = Vec::new();

        for v in self.cognomi {
            tags.push(ExtractedTag {
                tag_type: "surname".into(),
                value: v,
                confidence: None,
            });
        }
        for v in self.nomi {
            tags.push(ExtractedTag {
                tag_type: "name".into(),
                value: v,
                confidence: None,
            });
        }
        for v in self.date {
            tags.push(ExtractedTag {
                tag_type: "date".into(),
                value: v,
                confidence: None,
            });
        }
        for v in self.localita {
            tags.push(ExtractedTag {
                tag_type: "location".into(),
                value: v,
                confidence: None,
            });
        }
        if let Some(v) = self.tipo_evento {
            tags.push(ExtractedTag {
                tag_type: "event_type".into(),
                value: v,
                confidence: None,
            });
        }
        for r in self.ruoli {
            tags.push(ExtractedTag {
                tag_type: "role".into(),
                value: format!("{}: {}", r.ruolo, r.nome),
                confidence: None,
            });
        }
        for v in self.professioni {
            tags.push(ExtractedTag {
                tag_type: "profession".into(),
                value: v,
                confidence: None,
            });
        }

        tags
    }
}
