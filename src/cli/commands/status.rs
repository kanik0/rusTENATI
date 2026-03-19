use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::download::state::StateDb;

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Show specific session
    #[arg(long)]
    pub session: Option<i64>,

    /// Show all sessions
    #[arg(long)]
    pub all: bool,

    /// Database path
    #[arg(long, default_value = "./antenati/rustenati.db")]
    pub db: PathBuf,
}

pub async fn run(args: &StatusArgs, json_output: bool) -> Result<()> {
    if !args.db.exists() {
        if json_output {
            println!(
                "{}",
                serde_json::json!({
                    "error": "No database found. Run a download first.",
                })
            );
        } else {
            println!("No database found at {}. Run a download first.", args.db.display());
        }
        return Ok(());
    }

    let db = StateDb::open(&args.db)?;

    if let Some(session_id) = args.session {
        return show_session(&db, session_id, json_output);
    }

    show_overview(&db, args.all, json_output)
}

fn show_overview(db: &StateDb, show_all: bool, json_output: bool) -> Result<()> {
    let global = db.get_global_stats()?;

    if json_output {
        let sessions = db.list_sessions()?;
        println!(
            "{}",
            serde_json::json!({
                "global": global,
                "sessions": sessions,
            })
        );
        return Ok(());
    }

    println!("Rustenati Status");
    println!("═══════════════════════════════════════");
    println!("Manifests:  {}", global.manifests);
    println!("Sessions:   {}", global.sessions);
    println!("Downloads:  {} total", global.total_downloads);
    println!(
        "            {} complete, {} failed, {} pending",
        global.complete, global.failed, global.pending
    );
    if global.total_downloads > 0 {
        let pct = (global.complete as f64 / global.total_downloads as f64) * 100.0;
        println!("            {pct:.1}% complete");
    }
    println!("Tags:       {}", global.tags);

    let sessions = db.list_sessions()?;
    if sessions.is_empty() {
        return Ok(());
    }

    let display_sessions = if show_all {
        &sessions[..]
    } else {
        &sessions[..sessions.len().min(10)]
    };

    println!();
    if !show_all && sessions.len() > 10 {
        println!("Recent sessions (showing 10/{}, use --all for all):", sessions.len());
    } else {
        println!("Sessions:");
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["ID", "Started", "Status", "Title", "Progress"]);

    for s in display_sessions {
        let title = s
            .title
            .as_deref()
            .map(|t| {
                if t.len() > 40 {
                    format!("{}...", &t[..37])
                } else {
                    t.to_string()
                }
            })
            .unwrap_or_else(|| s.manifest_id.clone());

        let progress = if s.total_canvases > 0 {
            format!(
                "{}/{} ({:.0}%)",
                s.completed,
                s.total_canvases,
                (s.completed as f64 / s.total_canvases as f64) * 100.0
            )
        } else {
            format!("{} done", s.completed)
        };

        table.add_row(vec![
            &s.id.to_string(),
            &s.started_at,
            &s.status,
            &title,
            &progress,
        ]);
    }

    println!("{table}");
    Ok(())
}

fn show_session(db: &StateDb, session_id: i64, json_output: bool) -> Result<()> {
    let session = db.get_session(session_id)?;

    let session = match session {
        Some(s) => s,
        None => {
            anyhow::bail!("Session {session_id} not found");
        }
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&session)?);
        return Ok(());
    }

    println!("Session #{}", session.id);
    println!("─────────────────────────────────────");
    println!("Started:    {}", session.started_at);
    println!("Status:     {}", session.status);
    println!("Manifest:   {}", session.manifest_id);
    if let Some(title) = &session.title {
        println!("Title:      {title}");
    }
    println!("Canvases:   {}", session.total_canvases);
    println!("Completed:  {}", session.completed);
    println!("Failed:     {}", session.failed);
    let pending = session.total_canvases.saturating_sub(session.completed + session.failed);
    println!("Pending:    {pending}");
    if session.total_canvases > 0 {
        let pct = (session.completed as f64 / session.total_canvases as f64) * 100.0;
        println!("Progress:   {pct:.1}%");
    }

    Ok(())
}
