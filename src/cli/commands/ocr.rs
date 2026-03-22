use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Semaphore;

use crate::config::OcrConfig;
use crate::ocr::{self, DocumentType, OcrResult};

#[derive(Debug, Args)]
pub struct OcrArgs {
    /// Path to image file or directory of images
    pub path: PathBuf,

    /// OCR backend: claude, transkribus, azure, google
    #[arg(long)]
    pub backend: Option<String>,

    /// Language hint (it, la)
    #[arg(long, default_value = "it")]
    pub language: String,

    /// Parallel OCR requests
    #[arg(short, long, default_value = "2")]
    pub jobs: usize,

    /// Document type hint: birth, death, marriage
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Extract structured tags from OCR results
    #[arg(long)]
    pub extract_tags: bool,

    /// Save OCR output alongside images (as .txt / .json)
    #[arg(long, default_value = "true")]
    pub save: bool,

    /// Enhance images before OCR (contrast + denoise)
    #[arg(long)]
    pub enhance: bool,

    /// Enable binarization in enhancement (aggressive, may hurt some docs)
    #[arg(long)]
    pub binarize: bool,
}

pub async fn run(args: &OcrArgs, json_output: bool, ocr_config: &OcrConfig) -> Result<()> {
    let backend = ocr::create_backend(ocr_config, args.backend.as_deref())?;

    let doc_type = match args.doc_type.as_deref() {
        Some("birth" | "nascita" | "nati") => DocumentType::Birth,
        Some("death" | "morte" | "morti") => DocumentType::Death,
        Some("marriage" | "matrimonio" | "matrimoni") => DocumentType::Marriage,
        _ => DocumentType::Unknown,
    };

    // Collect image files
    let images = collect_images(&args.path)?;
    if images.is_empty() {
        anyhow::bail!("No image files found in {}", args.path.display());
    }

    eprintln!(
        "OCR: {} images with backend '{}' (jobs={})",
        images.len(),
        backend.name(),
        args.jobs
    );

    let pb = ProgressBar::new(images.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let semaphore = std::sync::Arc::new(Semaphore::new(args.jobs));
    let backend = std::sync::Arc::new(backend);
    let mut handles = Vec::new();

    let enhancer = if args.enhance {
        Some(std::sync::Arc::new(crate::ocr::enhance::ImageEnhancer {
            contrast: true,
            denoise: true,
            binarize: args.binarize,
        }))
    } else {
        None
    };

    if args.enhance {
        eprintln!(
            "Image enhancement enabled (contrast+denoise{})",
            if args.binarize { "+binarize" } else { "" }
        );
    }

    for image_path in &images {
        let sem = semaphore.clone();
        let backend = backend.clone();
        let language = args.language.clone();
        let extract_tags = args.extract_tags;
        let save = args.save;
        let image_path = image_path.clone();
        let pb = pb.clone();
        let enhancer = enhancer.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let file_name = image_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            pb.set_message(file_name.clone());

            // Enhance image if requested, saving to temp file
            let (ocr_path, _temp_file) = if let Some(ref enh) = enhancer {
                let tmp = std::env::temp_dir().join(format!("rustenati_enh_{file_name}"));
                match enh.enhance(&image_path, &tmp) {
                    Ok(()) => (tmp.clone(), Some(tmp)),
                    Err(e) => {
                        tracing::warn!("Enhancement failed for {file_name}, using original: {e}");
                        (image_path.clone(), None)
                    }
                }
            } else {
                (image_path.clone(), None)
            };

            let result = backend
                .recognize(&ocr_path, &language, doc_type, extract_tags)
                .await;

            match &result {
                Ok(ocr_result) if save => {
                    if let Err(e) = save_result(&image_path, ocr_result, extract_tags) {
                        tracing::warn!("Failed to save OCR result for {file_name}: {e}");
                    }
                }
                _ => {}
            }

            // Clean up temp enhanced file
            if let Some(ref tmp) = _temp_file {
                let _ = std::fs::remove_file(tmp);
            }

            pb.inc(1);
            (image_path, result)
        });

        handles.push(handle);
    }

    let mut results: Vec<(PathBuf, Result<OcrResult>)> = Vec::new();
    for handle in handles {
        let (path, result) = handle.await.context("OCR task panicked")?;
        results.push((path, result));
    }

    pb.finish_and_clear();

    // Output results
    let succeeded: Vec<_> = results
        .iter()
        .filter(|(_, r)| r.is_ok())
        .collect();
    let failed: Vec<_> = results
        .iter()
        .filter(|(_, r)| r.is_err())
        .collect();

    if json_output {
        let json_results: Vec<_> = results
            .iter()
            .map(|(path, r)| {
                serde_json::json!({
                    "path": path.display().to_string(),
                    "success": r.is_ok(),
                    "result": r.as_ref().ok(),
                    "error": r.as_ref().err().map(|e| e.to_string()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_results)?);
    } else {
        // Print each transcription
        for (path, result) in &results {
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            match result {
                Ok(ocr_result) => {
                    eprintln!("--- {file_name} ---");
                    println!("{}", ocr_result.text);

                    if !ocr_result.tags.is_empty() {
                        eprintln!("\nTag estratti:");
                        for tag in &ocr_result.tags {
                            eprintln!("  [{:>12}] {}", tag.tag_type, tag.value);
                        }
                    }
                    println!();
                }
                Err(e) => {
                    eprintln!("FAILED {file_name}: {e}");
                }
            }
        }

        eprintln!(
            "\nOCR complete: {} succeeded, {} failed",
            succeeded.len(),
            failed.len()
        );
    }

    Ok(())
}

/// Collect image files from a path (file or directory).
fn collect_images(path: &std::path::Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut images = Vec::new();
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("Failed to read directory: {}", path.display()))?
    {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                match ext.to_lowercase().as_str() {
                    "jpg" | "jpeg" | "png" | "gif" | "webp" | "tiff" | "tif" | "bmp" => {
                        images.push(p);
                    }
                    _ => {}
                }
            }
        }
    }

    images.sort();
    Ok(images)
}

/// Save OCR result as .txt (and optionally .json with tags) alongside the image.
fn save_result(image_path: &std::path::Path, result: &OcrResult, include_tags: bool) -> Result<()> {
    let stem = image_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let parent = image_path.parent().unwrap_or(std::path::Path::new("."));

    // Determine OCR output directory (sibling "ocr/" directory if images are in "images/")
    let ocr_dir = if parent.ends_with("images") {
        parent.parent().unwrap_or(parent).join("ocr")
    } else {
        parent.join("ocr")
    };

    std::fs::create_dir_all(&ocr_dir)
        .with_context(|| format!("Failed to create OCR output dir: {}", ocr_dir.display()))?;

    // Save plain text transcription
    let txt_path = ocr_dir.join(format!("{stem}.txt"));
    std::fs::write(&txt_path, &result.text)
        .with_context(|| format!("Failed to write {}", txt_path.display()))?;

    // Save structured JSON if tags were extracted
    if include_tags && !result.tags.is_empty() {
        let json_path = ocr_dir.join(format!("{stem}.json"));
        let json = serde_json::to_string_pretty(result)?;
        std::fs::write(&json_path, json)
            .with_context(|| format!("Failed to write {}", json_path.display()))?;
    }

    Ok(())
}
