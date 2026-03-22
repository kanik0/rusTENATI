use std::io::Write;

use anyhow::Result;
use chrono::Utc;
use clap::Args;

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Export format: csv, json, gedcom
    #[arg(short, long, default_value = "csv")]
    pub format: String,

    /// What to export: manifests, downloads, tags, persons, registries, gedcom
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
        "gedcom" => export_gedcom(&state_db, &mut writer),
        other => anyhow::bail!("Unknown export type: '{other}'. Use: manifests, downloads, tags, persons, registries, gedcom"),
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

/// Export all persons and their linked records as GEDCOM 5.5.1 format.
/// GEDCOM is the universal standard for genealogy software (FamilySearch, Ancestry, MyHeritage, Gramps).
fn export_gedcom(db: &StateDb, w: &mut dyn Write) -> Result<()> {
    let persons = db.get_all_persons_full()?;

    if persons.is_empty() {
        anyhow::bail!("No persons found in database. Run name searches first to populate person records.");
    }

    // GEDCOM Header
    writeln!(w, "0 HEAD")?;
    writeln!(w, "1 SOUR RUSTENATI")?;
    writeln!(w, "2 VERS {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(w, "2 NAME Rustenati")?;
    writeln!(w, "2 CORP Portale Antenati Dumper")?;
    writeln!(w, "1 DEST ANY")?;
    writeln!(w, "1 DATE {}", Utc::now().format("%d %b %Y").to_string().to_uppercase())?;
    writeln!(w, "1 SUBM @SUBM1@")?;
    writeln!(w, "1 GEDC")?;
    writeln!(w, "2 VERS 5.5.1")?;
    writeln!(w, "2 FORM LINEAGE-LINKED")?;
    writeln!(w, "1 CHAR UTF-8")?;
    writeln!(w, "1 LANG Italian")?;

    // Submitter record
    writeln!(w, "0 @SUBM1@ SUBM")?;
    writeln!(w, "1 NAME Rustenati User")?;

    // Individual records
    for person in &persons {
        let indi_id = format!("@I{}@", person.id);
        writeln!(w, "0 {} INDI", indi_id)?;

        // Name
        let surname = person.surname.as_deref().unwrap_or("");
        let given = person.given_name.as_deref().unwrap_or("");
        if !surname.is_empty() || !given.is_empty() {
            writeln!(w, "1 NAME {} /{}/", given, surname)?;
            if !surname.is_empty() {
                writeln!(w, "2 SURN {}", surname)?;
            }
            if !given.is_empty() {
                writeln!(w, "2 GIVN {}", given)?;
            }
        }

        // Birth event
        if person.birth_year.is_some() || person.birth_place.is_some() || person.birth_info.is_some() {
            writeln!(w, "1 BIRT")?;
            if let Some(year) = person.birth_year {
                writeln!(w, "2 DATE {}", year)?;
            }
            if let Some(ref place) = person.birth_place {
                if !place.is_empty() {
                    writeln!(w, "2 PLAC {}", place)?;
                }
            }
            if let Some(ref info) = person.birth_info {
                if !info.is_empty() {
                    writeln!(w, "2 NOTE {}", gedcom_escape(info))?;
                }
            }
        }

        // Death event
        if person.death_year.is_some() || person.death_place.is_some() || person.death_info.is_some() {
            writeln!(w, "1 DEAT")?;
            if let Some(year) = person.death_year {
                writeln!(w, "2 DATE {}", year)?;
            }
            if let Some(ref place) = person.death_place {
                if !place.is_empty() {
                    writeln!(w, "2 PLAC {}", place)?;
                }
            }
            if let Some(ref info) = person.death_info {
                if !info.is_empty() {
                    writeln!(w, "2 NOTE {}", gedcom_escape(info))?;
                }
            }
        }

        // Source citations from linked records
        let records = db.get_person_records(person.id)?;
        for record in &records {
            if let Some(ref ark) = record.ark_url {
                writeln!(w, "1 SOUR @S_ANTENATI@")?;
                writeln!(w, "2 PAGE {}", ark)?;
                if let Some(ref date) = record.date {
                    writeln!(w, "2 DATA")?;
                    writeln!(w, "3 DATE {}", date)?;
                }
                if let Some(ref rtype) = record.record_type {
                    writeln!(w, "2 NOTE Record type: {}", rtype)?;
                }
            }
        }

        // Link to Portale Antenati detail page
        if let Some(ref url) = person.detail_url {
            writeln!(w, "1 NOTE Portale Antenati: {}", url)?;
        }
    }

    // Source record for Portale Antenati
    writeln!(w, "0 @S_ANTENATI@ SOUR")?;
    writeln!(w, "1 TITL Portale Antenati - Gli Archivi per la ricerca anagrafica")?;
    writeln!(w, "1 AUTH Ministero della Cultura, Italia")?;
    writeln!(w, "1 PUBL https://antenati.cultura.gov.it/")?;
    writeln!(w, "1 REPO @R_ANTENATI@")?;

    // Repository record
    writeln!(w, "0 @R_ANTENATI@ REPO")?;
    writeln!(w, "1 NAME Portale Antenati")?;
    writeln!(w, "1 ADDR https://antenati.cultura.gov.it/")?;

    // Trailer
    writeln!(w, "0 TRLR")?;

    eprintln!("Exported {} persons to GEDCOM 5.5.1 format", persons.len());
    Ok(())
}

/// Escape text for GEDCOM (line breaks become CONT records).
fn gedcom_escape(s: &str) -> String {
    s.replace('\n', " ").replace('\r', "")
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
