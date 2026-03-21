use std::io::Write;

use anyhow::Result;
use clap::Args;

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Export format: csv, json
    #[arg(short, long, default_value = "csv")]
    pub format: String,

    /// What to export: manifests, downloads, tags, persons, registries
    pub what: String,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Filter by manifest ID
    #[arg(long)]
    pub manifest: Option<String>,

    /// Filter by doc type
    #[arg(long)]
    pub doc_type: Option<String>,

    /// Filter by year
    #[arg(long)]
    pub year: Option<String>,
}

pub fn run(args: &ExportArgs) -> Result<()> {
    let state_db = StateDb::open(&output::db_path())?;

    let mut writer: Box<dyn Write> = if let Some(ref path) = args.output {
        Box::new(std::fs::File::create(path)?)
    } else {
        Box::new(std::io::stdout())
    };

    match args.what.as_str() {
        "manifests" => export_manifests(&state_db, args, &mut writer),
        "downloads" => export_downloads(&state_db, args, &mut writer),
        "tags" => export_tags(&state_db, args, &mut writer),
        "persons" => export_persons(&state_db, args, &mut writer),
        "registries" => export_registries(&state_db, args, &mut writer),
        other => anyhow::bail!("Unknown export type: '{other}'. Use: manifests, downloads, tags, persons, registries"),
    }
}

fn export_manifests(db: &StateDb, args: &ExportArgs, w: &mut dyn Write) -> Result<()> {
    let records = db.search_manifests(
        args.doc_type.as_deref(),
        args.year.as_deref(),
        None,
        None,
    )?;

    match args.format.as_str() {
        "json" => {
            writeln!(w, "{}", serde_json::to_string_pretty(&records)?)?;
        }
        "csv" => {
            writeln!(w, "id,title,doc_type,year,archive,signature")?;
            for r in &records {
                writeln!(w, "{},{},{},{},{},{}",
                    csv_escape(&r.id),
                    csv_escape(r.title.as_deref().unwrap_or("")),
                    csv_escape(r.doc_type.as_deref().unwrap_or("")),
                    csv_escape(r.year.as_deref().unwrap_or("")),
                    csv_escape(r.archive_name.as_deref().unwrap_or("")),
                    csv_escape(r.signature.as_deref().unwrap_or("")),
                )?;
            }
        }
        f => anyhow::bail!("Unsupported format: {f}"),
    }
    Ok(())
}

fn export_downloads(db: &StateDb, args: &ExportArgs, w: &mut dyn Write) -> Result<()> {
    let downloads = db.get_completed_downloads(args.manifest.as_deref())?;

    match args.format.as_str() {
        "json" => {
            let json: Vec<serde_json::Value> = downloads.iter().map(|d| {
                serde_json::json!({
                    "manifest_id": d.manifest_id,
                    "canvas_id": d.canvas_id,
                    "local_path": d.local_path,
                    "sha256": d.sha256,
                })
            }).collect();
            writeln!(w, "{}", serde_json::to_string_pretty(&json)?)?;
        }
        "csv" => {
            writeln!(w, "manifest_id,canvas_id,local_path,sha256")?;
            for d in &downloads {
                writeln!(w, "{},{},{},{}",
                    csv_escape(&d.manifest_id),
                    csv_escape(&d.canvas_id),
                    csv_escape(&d.local_path),
                    csv_escape(&d.sha256),
                )?;
            }
        }
        f => anyhow::bail!("Unsupported format: {f}"),
    }
    Ok(())
}

fn export_tags(db: &StateDb, _args: &ExportArgs, w: &mut dyn Write) -> Result<()> {
    let tags = db.search_tags(None, None)?;

    writeln!(w, "tag_type,value,confidence,manifest_id,canvas_id")?;
    for t in &tags {
        writeln!(w, "{},{},{},{},{}",
            csv_escape(&t.tag_type),
            csv_escape(&t.value),
            t.confidence.map_or(String::new(), |c| format!("{:.2}", c)),
            csv_escape(&t.manifest_id),
            csv_escape(&t.canvas_id),
        )?;
    }
    Ok(())
}

fn export_persons(db: &StateDb, _args: &ExportArgs, w: &mut dyn Write) -> Result<()> {
    let persons = db.search_persons(None, None)?;

    match _args.format.as_str() {
        "json" => {
            writeln!(w, "{}", serde_json::to_string_pretty(&persons)?)?;
        }
        "csv" => {
            writeln!(w, "id,name,surname,given_name,birth_info,death_info")?;
            for p in &persons {
                writeln!(w, "{},{},{},{},{},{}",
                    p.id,
                    csv_escape(&p.name),
                    csv_escape(p.surname.as_deref().unwrap_or("")),
                    csv_escape(p.given_name.as_deref().unwrap_or("")),
                    csv_escape(p.birth_info.as_deref().unwrap_or("")),
                    csv_escape(p.death_info.as_deref().unwrap_or("")),
                )?;
            }
        }
        f => anyhow::bail!("Unsupported format: {f}"),
    }
    Ok(())
}

fn export_registries(db: &StateDb, args: &ExportArgs, w: &mut dyn Write) -> Result<()> {
    let (records, _) = db.search_registries_catalog(
        args.doc_type.as_deref(),
        args.year.as_deref(),
        None,
        None,
        None,
        0,
        100_000,
    )?;

    match args.format.as_str() {
        "json" => {
            writeln!(w, "{}", serde_json::to_string_pretty(&records)?)?;
        }
        "csv" => {
            writeln!(w, "ark_url,year,doc_type,archive,locality,province,has_images")?;
            for r in &records {
                writeln!(w, "{},{},{},{},{},{},{}",
                    csv_escape(&r.ark_url),
                    csv_escape(r.year.as_deref().unwrap_or("")),
                    csv_escape(r.doc_type.as_deref().unwrap_or("")),
                    csv_escape(r.archive_name.as_deref().unwrap_or("")),
                    csv_escape(r.locality_name.as_deref().unwrap_or("")),
                    csv_escape(r.province.as_deref().unwrap_or("")),
                    r.has_images,
                )?;
            }
        }
        f => anyhow::bail!("Unsupported format: {f}"),
    }
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
