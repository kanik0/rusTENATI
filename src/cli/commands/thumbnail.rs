use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct ThumbnailArgs {
    /// Maximum thumbnail width in pixels
    #[arg(short = 'W', long, default_value = "200")]
    pub width: u32,

    /// Maximum thumbnail height in pixels
    #[arg(short = 'H', long, default_value = "200")]
    pub height: u32,

    /// Only process thumbnails for a specific manifest
    #[arg(long)]
    pub manifest: Option<String>,

    /// Force regeneration of existing thumbnails
    #[arg(long)]
    pub force: bool,

    /// JPEG quality (1-100)
    #[arg(long, default_value = "80")]
    pub quality: u8,
}

pub fn run(args: &ThumbnailArgs) -> Result<()> {
    let state_db = StateDb::open(&output::db_path())?;

    let downloads = state_db.get_completed_downloads(args.manifest.as_deref())?;

    if downloads.is_empty() {
        eprintln!("No completed downloads found.");
        return Ok(());
    }

    let bar = ProgressBar::new(downloads.len() as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut generated = 0usize;
    let mut skipped = 0usize;
    let mut errors = 0usize;

    for dl in &downloads {
        bar.inc(1);

        let source = Path::new(&dl.local_path);
        if !source.exists() {
            continue;
        }

        let thumb_path = thumbnail_path(source);

        // Skip if thumbnail exists and not forcing
        if !args.force && thumb_path.exists() {
            skipped += 1;
            continue;
        }

        // Create thumbnail directory
        if let Some(parent) = thumb_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        match generate_thumbnail(source, &thumb_path, args.width, args.height, args.quality) {
            Ok(()) => generated += 1,
            Err(e) => {
                tracing::debug!("Failed to create thumbnail for {}: {e}", source.display());
                errors += 1;
            }
        }
    }

    bar.finish_with_message("done!");
    eprintln!(
        "Thumbnails: {} generated, {} skipped, {} errors (out of {} downloads)",
        generated, skipped, errors, downloads.len()
    );

    Ok(())
}

/// Compute thumbnail path: replace /images/ with /thumbnails/ in the path.
fn thumbnail_path(source: &Path) -> PathBuf {
    let source_str = source.to_string_lossy();
    if source_str.contains("/images/") {
        let thumb_str = source_str.replacen("/images/", "/thumbnails/", 1);
        PathBuf::from(thumb_str)
    } else {
        // Fallback: put thumbnail next to source with _thumb suffix
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let ext = source.extension().unwrap_or_default().to_string_lossy();
        source.with_file_name(format!("{stem}_thumb.{ext}"))
    }
}

/// Generate a thumbnail using the `image` crate.
fn generate_thumbnail(
    source: &Path,
    dest: &Path,
    max_width: u32,
    max_height: u32,
    quality: u8,
) -> Result<()> {
    let img = image::open(source)?;
    let thumb = img.thumbnail(max_width, max_height);

    // Save as JPEG for smallest file size
    let dest_jpg = dest.with_extension("jpg");
    let mut output = std::fs::File::create(&dest_jpg)?;
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality);
    thumb.write_with_encoder(encoder)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thumbnail_path() {
        let p = Path::new("/data/antenati/archive/images/001.jpg");
        assert_eq!(
            thumbnail_path(p),
            PathBuf::from("/data/antenati/archive/thumbnails/001.jpg")
        );
    }

    #[test]
    fn test_thumbnail_path_fallback() {
        let p = Path::new("/data/other/001.jpg");
        assert_eq!(
            thumbnail_path(p),
            PathBuf::from("/data/other/001_thumb.jpg")
        );
    }
}
