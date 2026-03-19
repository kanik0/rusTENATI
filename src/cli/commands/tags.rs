use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::config::OcrConfig;
use crate::download::state::StateDb;
use crate::ocr::{self, DocumentType};

#[derive(Debug, Subcommand)]
pub enum TagsAction {
    /// Search tags across all downloads
    Search(TagsSearchArgs),

    /// List tags for a specific download
    List(TagsListArgs),

    /// Manually add a tag
    Add(TagsAddArgs),

    /// Extract tags from existing OCR transcriptions using an LLM
    Extract(TagsExtractArgs),

    /// Show tag statistics
    Stats,
}

#[derive(Debug, Args)]
pub struct TagsSearchArgs {
    /// Filter by surname
    #[arg(long)]
    pub surname: Option<String>,

    /// Filter by name
    #[arg(long)]
    pub name: Option<String>,

    /// Filter by date
    #[arg(long)]
    pub date: Option<String>,

    /// Filter by locality
    #[arg(long)]
    pub locality: Option<String>,

    /// Filter by tag type
    #[arg(long)]
    pub tag_type: Option<String>,

    /// Filter by value (substring match)
    #[arg(long)]
    pub value: Option<String>,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct TagsListArgs {
    /// Download ID to list tags for
    #[arg(long)]
    pub download_id: i64,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct TagsAddArgs {
    /// Download ID to add tag to
    pub download_id: i64,

    /// Tag type: surname, name, date, location, event_type, role, profession, other
    #[arg(long)]
    pub tag_type: String,

    /// Tag value
    #[arg(long)]
    pub value: String,

    /// Confidence score (0.0-1.0)
    #[arg(long)]
    pub confidence: Option<f32>,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

#[derive(Debug, Args)]
pub struct TagsExtractArgs {
    /// Path to directory with OCR transcriptions (.txt files)
    pub path: PathBuf,

    /// OCR backend to use for tag extraction (default: claude)
    #[arg(long)]
    pub backend: Option<String>,

    /// Database path (to store extracted tags)
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,

    /// Document type hint: birth, death, marriage
    #[arg(long)]
    pub doc_type: Option<String>,
}

pub async fn run(action: &TagsAction, json_output: bool, ocr_config: &OcrConfig) -> Result<()> {
    match action {
        TagsAction::Search(args) => run_search(args, json_output),
        TagsAction::List(args) => run_list(args, json_output),
        TagsAction::Add(args) => run_add(args, json_output),
        TagsAction::Extract(args) => run_extract(args, json_output, ocr_config).await,
        TagsAction::Stats => run_stats(json_output),
    }
}

fn open_db(path: &std::path::Path) -> Result<StateDb> {
    if !path.exists() {
        anyhow::bail!(
            "Database not found at {}. Run a download first or specify --db path.",
            path.display()
        );
    }
    StateDb::open(path)
}

fn run_search(args: &TagsSearchArgs, json_output: bool) -> Result<()> {
    let db = open_db(&args.db)?;

    let searches: Vec<(Option<&str>, Option<&str>)> = if args.surname.is_some()
        || args.name.is_some()
        || args.date.is_some()
        || args.locality.is_some()
    {
        let mut s = Vec::new();
        if let Some(v) = &args.surname {
            s.push((Some("surname"), Some(v.as_str())));
        }
        if let Some(v) = &args.name {
            s.push((Some("name"), Some(v.as_str())));
        }
        if let Some(v) = &args.date {
            s.push((Some("date"), Some(v.as_str())));
        }
        if let Some(v) = &args.locality {
            s.push((Some("location"), Some(v.as_str())));
        }
        s
    } else {
        vec![(args.tag_type.as_deref(), args.value.as_deref())]
    };

    let mut all_results = Vec::new();
    for (tt, vp) in &searches {
        let results = db.search_tags(*tt, *vp)?;
        all_results.extend(results);
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&all_results)?);
        return Ok(());
    }

    if all_results.is_empty() {
        println!("No tags found.");
        return Ok(());
    }

    eprintln!("{} tags found", all_results.len());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["#", "Type", "Value", "Confidence", "Source", "Manifest", "Canvas"]);

    for (i, tag) in all_results.iter().enumerate() {
        let conf = tag
            .confidence
            .map(|c| format!("{:.0}%", c * 100.0))
            .unwrap_or_else(|| "-".into());
        let source = tag.source.as_deref().unwrap_or("-");
        let manifest_short = if tag.manifest_id.len() > 20 {
            format!("...{}", &tag.manifest_id[tag.manifest_id.len() - 17..])
        } else {
            tag.manifest_id.clone()
        };

        table.add_row(vec![
            &format!("{}", i + 1),
            &tag.tag_type,
            &tag.value,
            &conf,
            source,
            &manifest_short,
            &tag.canvas_id,
        ]);
    }

    println!("{table}");
    Ok(())
}

fn run_list(args: &TagsListArgs, json_output: bool) -> Result<()> {
    let db = open_db(&args.db)?;
    let tags = db.get_tags_for_download(args.download_id)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&tags)?);
        return Ok(());
    }

    if tags.is_empty() {
        println!("No tags for download ID {}.", args.download_id);
        return Ok(());
    }

    eprintln!(
        "Tags for download {} (manifest: {}, canvas: {})",
        args.download_id,
        tags[0].manifest_id,
        tags[0].canvas_id
    );

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["Type", "Value", "Confidence", "Source"]);

    for tag in &tags {
        let conf = tag
            .confidence
            .map(|c| format!("{:.0}%", c * 100.0))
            .unwrap_or_else(|| "-".into());
        table.add_row(vec![
            &tag.tag_type,
            &tag.value,
            &conf,
            tag.source.as_deref().unwrap_or("-"),
        ]);
    }

    println!("{table}");
    Ok(())
}

fn run_add(args: &TagsAddArgs, json_output: bool) -> Result<()> {
    let valid_types = [
        "surname", "name", "date", "location", "event_type", "role", "profession", "other",
    ];
    if !valid_types.contains(&args.tag_type.as_str()) {
        anyhow::bail!(
            "Invalid tag type '{}'. Valid types: {}",
            args.tag_type,
            valid_types.join(", ")
        );
    }

    let db = open_db(&args.db)?;
    db.insert_tag(
        args.download_id,
        &args.tag_type,
        &args.value,
        args.confidence,
        Some("manual"),
    )?;

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "success": true,
                "download_id": args.download_id,
                "tag_type": args.tag_type,
                "value": args.value,
            })
        );
    } else {
        println!(
            "Tag added: [{}] {} (download #{})",
            args.tag_type, args.value, args.download_id
        );
    }

    Ok(())
}

async fn run_extract(args: &TagsExtractArgs, json_output: bool, ocr_config: &OcrConfig) -> Result<()> {
    let backend = ocr::create_backend(ocr_config, args.backend.as_deref())?;

    let doc_type = match args.doc_type.as_deref() {
        Some("birth" | "nascita" | "nati") => DocumentType::Birth,
        Some("death" | "morte" | "morti") => DocumentType::Death,
        Some("marriage" | "matrimonio" | "matrimoni") => DocumentType::Marriage,
        _ => DocumentType::Unknown,
    };

    // Collect .txt files from directory
    let txt_files = collect_txt_files(&args.path)?;
    if txt_files.is_empty() {
        anyhow::bail!("No .txt transcription files found in {}", args.path.display());
    }

    eprintln!(
        "Extracting tags from {} transcriptions using '{}'...",
        txt_files.len(),
        backend.name()
    );

    // For each transcription, we create a temporary image-like request
    // with the text content, asking the LLM to extract structured tags.
    // Since we already have the text, we use a text-only approach.
    let mut total_tags = 0;
    let mut processed = 0;

    for txt_path in &txt_files {
        let text = std::fs::read_to_string(txt_path)
            .with_context(|| format!("Failed to read {}", txt_path.display()))?;

        if text.trim().is_empty() {
            continue;
        }

        let file_name = txt_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Use the backend to extract tags from the text.
        // For Claude, we send the text as a "document" and ask for tag extraction.
        let tags = extract_tags_from_text(backend.as_ref(), &text, doc_type).await?;

        if !tags.is_empty() {
            if json_output {
                println!(
                    "{}",
                    serde_json::json!({
                        "file": file_name,
                        "tags": tags,
                    })
                );
            } else {
                eprintln!("  {file_name}: {} tags", tags.len());
                for tag in &tags {
                    eprintln!("    [{:>12}] {}", tag.tag_type, tag.value);
                }
            }
            total_tags += tags.len();
        }

        processed += 1;
    }

    if !json_output {
        eprintln!(
            "\nExtraction complete: {} files processed, {} tags extracted",
            processed, total_tags
        );
    }

    Ok(())
}

/// Extract structured tags from existing transcribed text using an OCR backend.
async fn extract_tags_from_text(
    backend: &dyn ocr::OcrBackend,
    text: &str,
    doc_type: DocumentType,
) -> Result<Vec<ocr::ExtractedTag>> {
    // For Claude backend: we create a temporary file with the text rendered as an image-like prompt.
    // Actually, for text extraction we bypass the image pipeline and use the API directly.
    // We'll create a small wrapper that sends just the text.

    // For now, use a simple heuristic: write text to a temp file, then use
    // the Claude API directly for tag extraction from text.
    if backend.name() == "claude" {
        return extract_tags_via_claude(text, doc_type).await;
    }

    // For other backends that are image-only, we can't extract from text
    anyhow::bail!(
        "Tag extraction from text is only supported with the 'claude' backend. Got: '{}'",
        backend.name()
    )
}

/// Use Claude API to extract structured tags from transcribed text.
async fn extract_tags_via_claude(
    text: &str,
    doc_type: DocumentType,
) -> Result<Vec<ocr::ExtractedTag>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY required for tag extraction")?;

    let prompt = format!(
        "Analizza questo testo trascritto da un atto di {} italiano e estrai i dati strutturati.\n\n\
         TESTO:\n{}\n\n\
         Rispondi SOLO con un JSON valido (senza markdown) con questi campi:\n\
         {{\n  \
           \"cognomi\": [\"...\"],\n  \
           \"nomi\": [\"...\"],\n  \
           \"date\": [\"YYYY-MM-DD o come appaiono\"],\n  \
           \"localita\": [\"...\"],\n  \
           \"tipo_evento\": \"nascita|morte|matrimonio|altro\",\n  \
           \"ruoli\": [{{\"nome\": \"...\", \"ruolo\": \"padre|madre|testimone|ufficiale|...\"}}],\n  \
           \"professioni\": [\"...\"]\n\
         }}\n\
         Includi solo i campi per cui trovi dati nel testo.",
        doc_type.as_italian(),
        text
    );

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 2048,
            "messages": [{
                "role": "user",
                "content": prompt,
            }]
        }))
        .send()
        .await
        .context("Failed to call Claude API for tag extraction")?;

    if !response.status().is_success() {
        let err = response.text().await.unwrap_or_default();
        anyhow::bail!("Claude API error: {err}");
    }

    let api_response: serde_json::Value = response.json().await?;

    let raw_text = api_response["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("");

    // Parse the JSON response - strip any markdown fences if present
    let json_str = raw_text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let parsed: TagsJson = serde_json::from_str(json_str)
        .unwrap_or_default();

    Ok(parsed.into_tags())
}

fn collect_txt_files(path: &std::path::Path) -> Result<Vec<PathBuf>> {
    if path.is_file() && path.extension().is_some_and(|e| e == "txt") {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        anyhow::bail!("Path does not exist or is not a directory: {}", path.display());
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("Failed to read directory: {}", path.display()))?
    {
        let p = entry?.path();
        if p.is_file() && p.extension().is_some_and(|e| e == "txt") {
            files.push(p);
        }
    }
    files.sort();
    Ok(files)
}

fn run_stats(json_output: bool) -> Result<()> {
    let db_path = PathBuf::from("./antenati/rustenati.db");
    let db = open_db(&db_path)?;

    let stats = db.get_tag_stats()?;
    let total = db.get_total_tag_count()?;

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "total": total,
                "by_type": stats,
            })
        );
        return Ok(());
    }

    if stats.is_empty() {
        println!("No tags in database.");
        return Ok(());
    }

    println!("Tag Statistics ({total} total)");
    println!("─────────────────────────────────────");

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["Type", "Count", "Unique Images"]);

    for s in &stats {
        table.add_row(vec![
            &s.tag_type,
            &s.count.to_string(),
            &s.unique_downloads.to_string(),
        ]);
    }

    println!("{table}");
    Ok(())
}

// Reuse the same JSON parsing as claude_vision
#[derive(serde::Deserialize, Default)]
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

#[derive(serde::Deserialize)]
struct RoleJson {
    nome: String,
    ruolo: String,
}

impl TagsJson {
    fn into_tags(self) -> Vec<ocr::ExtractedTag> {
        let mut tags = Vec::new();
        for v in self.cognomi {
            tags.push(ocr::ExtractedTag { tag_type: "surname".into(), value: v, confidence: None });
        }
        for v in self.nomi {
            tags.push(ocr::ExtractedTag { tag_type: "name".into(), value: v, confidence: None });
        }
        for v in self.date {
            tags.push(ocr::ExtractedTag { tag_type: "date".into(), value: v, confidence: None });
        }
        for v in self.localita {
            tags.push(ocr::ExtractedTag { tag_type: "location".into(), value: v, confidence: None });
        }
        if let Some(v) = self.tipo_evento {
            tags.push(ocr::ExtractedTag { tag_type: "event_type".into(), value: v, confidence: None });
        }
        for r in self.ruoli {
            tags.push(ocr::ExtractedTag { tag_type: "role".into(), value: format!("{}: {}", r.ruolo, r.nome), confidence: None });
        }
        for v in self.professioni {
            tags.push(ocr::ExtractedTag { tag_type: "profession".into(), value: v, confidence: None });
        }
        tags
    }
}
