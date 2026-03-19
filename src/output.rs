use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::models::manifest::IiifManifest;

/// Sanitize a string for use as a filesystem name.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Build the output directory structure for a manifest.
///
/// Layout: `{base}/{archive}/{register}/`
pub fn build_output_dir(base: &Path, manifest: &IiifManifest) -> PathBuf {
    let archive = manifest
        .archival_context()
        .map(|s| sanitize_filename(s))
        .unwrap_or_else(|| "unknown_archive".to_string());

    let register = sanitize_filename(manifest.title());

    base.join(archive).join(register)
}

/// Create the output directory structure (images/ and ocr/ subdirs).
pub fn ensure_output_dirs(output_dir: &Path) -> Result<()> {
    let images_dir = output_dir.join("images");
    let ocr_dir = output_dir.join("ocr");

    std::fs::create_dir_all(&images_dir)
        .with_context(|| format!("Failed to create images dir: {}", images_dir.display()))?;
    std::fs::create_dir_all(&ocr_dir)
        .with_context(|| format!("Failed to create ocr dir: {}", ocr_dir.display()))?;

    Ok(())
}

/// Generate an image filename from canvas index and label.
///
/// Format: `{index:03}_{sanitized_label}.{format}`
pub fn image_filename(index: usize, label: &str, format: &str) -> String {
    let sanitized = sanitize_filename(label);
    let sanitized = if sanitized.is_empty() {
        "page".to_string()
    } else {
        sanitized
    };
    format!("{:03}_{}.{}", index + 1, sanitized, format)
}

/// Write the manifest JSON to the output directory.
pub fn write_manifest_json(output_dir: &Path, manifest: &IiifManifest) -> Result<()> {
    let path = output_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(manifest)?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write manifest.json: {}", path.display()))?;
    Ok(())
}

/// Write download metadata to the output directory.
pub fn write_metadata_json(
    output_dir: &Path,
    manifest: &IiifManifest,
    downloaded_at: &str,
) -> Result<()> {
    let metadata = serde_json::json!({
        "manifest_id": manifest.id,
        "archive": manifest.archival_context(),
        "register_title": manifest.title(),
        "document_type": manifest.doc_type(),
        "total_images": manifest.canvases.len(),
        "iiif_version": manifest.version.to_string(),
        "downloaded_at": downloaded_at,
        "rustenati_version": env!("CARGO_PKG_VERSION"),
    });

    let path = output_dir.join("metadata.json");
    let json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write metadata.json: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("Nati - 1807"), "Nati - 1807");
        assert_eq!(sanitize_filename("a/b\\c:d*e"), "a_b_c_d_e");
        assert_eq!(sanitize_filename("  spaces  "), "spaces");
    }

    #[test]
    fn test_image_filename() {
        assert_eq!(image_filename(0, "pag. 1", "jpg"), "001_pag. 1.jpg");
        assert_eq!(image_filename(99, "pag. 100", "png"), "100_pag. 100.png");
        assert_eq!(image_filename(0, "", "jpg"), "001_page.jpg");
    }
}
