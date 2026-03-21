use anyhow::Result;
use clap::Args;
use sha2::{Digest, Sha256};

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Only check file existence (skip SHA256 verification)
    #[arg(long)]
    pub quick: bool,

    /// Re-queue missing/corrupted files for re-download
    #[arg(long)]
    pub fix: bool,

    /// Limit verification to a specific manifest ID
    #[arg(long)]
    pub manifest: Option<String>,
}

pub fn run(args: &VerifyArgs, json_output: bool) -> Result<()> {
    let state_db = StateDb::open(&output::db_path())?;

    let downloads = if let Some(ref manifest_id) = args.manifest {
        state_db.get_completed_downloads(Some(manifest_id))?
    } else {
        state_db.get_completed_downloads(None)?
    };

    let total = downloads.len();
    if total == 0 {
        eprintln!("No completed downloads to verify.");
        return Ok(());
    }

    eprintln!("Verifying {} downloaded files...", total);

    let mut ok = 0usize;
    let mut missing = 0usize;
    let mut corrupted = 0usize;
    let mut no_checksum = 0usize;
    let mut files_to_fix: Vec<(String, String)> = Vec::new();

    for (i, dl) in downloads.iter().enumerate() {
        let path = std::path::Path::new(&dl.local_path);

        if !path.exists() {
            eprintln!("  MISSING: {}", dl.local_path);
            missing += 1;
            files_to_fix.push((dl.manifest_id.clone(), dl.canvas_id.clone()));
            continue;
        }

        if args.quick {
            // Quick mode: just check existence + non-zero size
            match std::fs::metadata(path) {
                Ok(meta) if meta.len() > 0 => ok += 1,
                _ => {
                    eprintln!("  EMPTY: {}", dl.local_path);
                    corrupted += 1;
                    files_to_fix.push((dl.manifest_id.clone(), dl.canvas_id.clone()));
                }
            }
        } else {
            // Full mode: verify SHA256
            if dl.sha256.is_empty() {
                no_checksum += 1;
                ok += 1; // count as OK if no checksum to verify against
                continue;
            }

            let data = std::fs::read(path)?;
            let checksum = hex::encode(Sha256::digest(&data));

            if checksum == dl.sha256 {
                ok += 1;
            } else {
                eprintln!("  CORRUPTED: {} (expected {}, got {})", dl.local_path, &dl.sha256[..8], &checksum[..8]);
                corrupted += 1;
                files_to_fix.push((dl.manifest_id.clone(), dl.canvas_id.clone()));
            }
        }

        if (i + 1) % 100 == 0 {
            eprint!("\r  Verified {}/{}", i + 1, total);
        }
    }
    eprintln!();

    // Fix mode: reset corrupted/missing to pending for re-download
    if args.fix && !files_to_fix.is_empty() {
        for (manifest_id, canvas_id) in &files_to_fix {
            state_db.mark_failed(manifest_id, canvas_id, "verify: file missing or corrupted")?;
            state_db.reset_failed_to_pending_single(manifest_id, canvas_id)?;
        }
        eprintln!("Reset {} files for re-download (use --resume to re-download)", files_to_fix.len());
    }

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "total": total,
                "ok": ok,
                "missing": missing,
                "corrupted": corrupted,
                "no_checksum": no_checksum,
            })
        );
    } else {
        println!("Verification complete:");
        println!("  Total: {total}");
        println!("  OK: {ok}");
        if missing > 0 { println!("  Missing: {missing}"); }
        if corrupted > 0 { println!("  Corrupted: {corrupted}"); }
        if no_checksum > 0 { println!("  No checksum: {no_checksum}"); }
    }

    Ok(())
}
