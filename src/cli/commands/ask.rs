use std::io::Write;

use anyhow::{Context, Result};
use clap::Args;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct AskArgs {
    /// Natural language question about your genealogical records
    pub question: String,

    /// Number of OCR documents to include as context (default: 10)
    #[arg(short = 'k', long, default_value = "10")]
    pub context: usize,

    /// Claude model to use
    #[arg(long, default_value = "claude-sonnet-4-6")]
    pub model: String,

    /// API key (or set ANTHROPIC_API_KEY env var)
    #[arg(long)]
    pub api_key: Option<String>,
}

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

pub async fn run(args: &AskArgs) -> Result<()> {
    let api_key = args.api_key.clone()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .context("ANTHROPIC_API_KEY not set. Set it via --api-key or ANTHROPIC_API_KEY env var.")?;

    let state_db = StateDb::open(&output::db_path())?;

    // Step 1: Search OCR results for relevant documents
    let ocr_results = state_db.search_ocr_text(&args.question, args.context)?;

    if ocr_results.is_empty() {
        eprintln!("No OCR results found in database. Run OCR on downloaded images first:");
        eprintln!("  rustenati ocr ./antenati/*/images/ --backend claude --extract-tags");
        return Ok(());
    }

    debug!("Found {} relevant OCR documents for context", ocr_results.len());

    // Step 2: Build context from OCR results
    let mut context_docs = String::new();
    for (i, result) in ocr_results.iter().enumerate() {
        context_docs.push_str(&format!(
            "\n--- Documento {} ---\n",
            i + 1
        ));
        if let Some(ref title) = result.manifest_title {
            if !title.is_empty() {
                context_docs.push_str(&format!("Registro: {}\n", title));
            }
        }
        if let Some(ref label) = result.canvas_label {
            context_docs.push_str(&format!("Pagina: {}\n", label));
        }
        context_docs.push_str(&format!("Testo:\n{}\n", result.snippet));
    }

    // Step 3: Also search for relevant persons
    let words: Vec<&str> = args.question.split_whitespace().collect();
    let mut person_context = String::new();

    // Try to find persons matching words in the question (simple heuristic: capitalized words)
    for word in &words {
        if word.len() > 2 && word.chars().next().map_or(false, |c| c.is_uppercase()) {
            if let Ok(persons) = state_db.search_persons(Some(word), None) {
                for p in persons.iter().take(5) {
                    person_context.push_str(&format!(
                        "\nPersona: {} (cognome: {}, nato: {})\n",
                        p.name,
                        p.surname.as_deref().unwrap_or("?"),
                        p.birth_info.as_deref().unwrap_or("?"),
                    ));
                }
            }
        }
    }

    // Step 4: Build the prompt
    let system_prompt = format!(
        "Sei un assistente genealogico esperto specializzato in registri di stato civile italiani \
         (nascita, morte, matrimonio) del XVIII-XIX secolo dal Portale Antenati.\n\n\
         Hai accesso ai seguenti documenti trascritti via OCR dal database locale dell'utente:\n\
         {context_docs}\n\
         {}\
         \nRispondi alla domanda dell'utente basandoti ESCLUSIVAMENTE sui documenti forniti.\n\
         - Cita sempre il documento specifico da cui trai le informazioni\n\
         - Se non trovi la risposta nei documenti, dillo chiaramente\n\
         - Usa l'italiano per la risposta\n\
         - Segnala eventuali incertezze con [?]\n\
         - Se noti connessioni familiari tra documenti diversi, evidenziale",
        if person_context.is_empty() {
            String::new()
        } else {
            format!("Persone note nel database:\n{person_context}\n")
        }
    );

    // Step 5: Call Claude API
    let client = Client::new();

    let request = ApiRequest {
        model: &args.model,
        max_tokens: 4096,
        system: Some(&system_prompt),
        messages: vec![ChatMessage {
            role: "user",
            content: &args.question,
        }],
        stream: true,
    };

    let response = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to connect to Anthropic API")?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error (HTTP {status}): {error_body}");
    }

    // Step 6: Stream the response
    let mut stdout = std::io::stdout();
    let mut stream = response.bytes_stream();

    use futures_util::StreamExt;
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading API stream")?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // Process SSE events
        while let Some(event_end) = buffer.find("\n\n") {
            let event = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(sse) = serde_json::from_str::<StreamEvent>(data) {
                        match sse {
                            StreamEvent::ContentBlockDelta { delta } => {
                                if let Delta::TextDelta { text } = delta {
                                    print!("{}", text);
                                    let _ = stdout.flush();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    println!();

    eprintln!(
        "\n[Basato su {} documenti OCR dal database locale]",
        ocr_results.len()
    );

    Ok(())
}

// --- API types ---

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum StreamEvent {
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { delta: Delta },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Delta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(other)]
    Other,
}
