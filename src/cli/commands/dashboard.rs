use std::io;
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Row, Table};
use ratatui::Terminal;

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Args)]
pub struct DashboardArgs {
    /// Refresh interval in seconds
    #[arg(short, long, default_value = "2")]
    pub refresh: u64,
}

pub fn run(args: &DashboardArgs) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, args);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, args: &DashboardArgs) -> Result<()> {
    loop {
        // Load fresh data each tick
        let data = match load_dashboard_data() {
            Ok(d) => d,
            Err(e) => {
                // Show error in TUI instead of crashing
                terminal.draw(|f| {
                    let area = f.area();
                    let msg = Paragraph::new(format!("Error loading data: {e}\n\nPress 'q' to exit."))
                        .block(Block::default().title("rusTENATI Dashboard").borders(Borders::ALL));
                    f.render_widget(msg, area);
                })?;

                if wait_for_quit(args.refresh)? {
                    return Ok(());
                }
                continue;
            }
        };

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),   // Title
                    Constraint::Length(5),   // Stats overview
                    Constraint::Length(3),   // Progress bar
                    Constraint::Min(10),     // Table
                    Constraint::Length(1),   // Footer
                ])
                .split(f.area());

            // Title
            let title = Paragraph::new(Line::from(vec![
                Span::styled("rusTENATI", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" Dashboard"),
            ]))
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(title, chunks[0]);

            // Stats
            let stats_text = vec![
                Line::from(vec![
                    Span::styled("Manifests: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("{}", data.total_manifests)),
                    Span::raw("  |  "),
                    Span::styled("Downloads: ", Style::default().fg(Color::Green)),
                    Span::raw(format!("{} complete", data.completed_downloads)),
                    Span::raw("  |  "),
                    Span::styled("Pending: ", Style::default().fg(Color::Blue)),
                    Span::raw(format!("{}", data.pending_downloads)),
                    Span::raw("  |  "),
                    Span::styled("Failed: ", Style::default().fg(Color::Red)),
                    Span::raw(format!("{}", data.failed_downloads)),
                ]),
                Line::from(vec![
                    Span::styled("Archives: ", Style::default().fg(Color::Magenta)),
                    Span::raw(format!("{}", data.total_archives)),
                    Span::raw("  |  "),
                    Span::styled("Registries: ", Style::default().fg(Color::Magenta)),
                    Span::raw(format!("{}", data.total_registries)),
                    Span::raw("  |  "),
                    Span::styled("Tags: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", data.total_tags)),
                    Span::raw("  |  "),
                    Span::styled("OCR: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", data.total_ocr)),
                ]),
                Line::from(vec![
                    Span::styled("Disk: ", Style::default().fg(Color::White)),
                    Span::raw(format_bytes(data.disk_usage)),
                ]),
            ];
            let stats = Paragraph::new(stats_text)
                .block(Block::default().title("Overview").borders(Borders::ALL));
            f.render_widget(stats, chunks[1]);

            // Progress bar
            let total = data.completed_downloads + data.pending_downloads + data.failed_downloads;
            let ratio = if total > 0 {
                data.completed_downloads as f64 / total as f64
            } else {
                0.0
            };
            let gauge = Gauge::default()
                .block(Block::default().title("Download Progress").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Green))
                .ratio(ratio)
                .label(format!(
                    "{}/{} ({:.1}%)",
                    data.completed_downloads,
                    total,
                    ratio * 100.0
                ));
            f.render_widget(gauge, chunks[2]);

            // Recent downloads table
            let header = Row::new(vec!["Manifest", "Doc Type", "Year", "Status", "Progress"])
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

            let rows: Vec<Row> = data
                .recent_manifests
                .iter()
                .map(|m| {
                    let progress = if m.total > 0 {
                        format!("{}/{}", m.completed, m.total)
                    } else {
                        "-".to_string()
                    };
                    let status_style = match m.status.as_str() {
                        "complete" => Style::default().fg(Color::Green),
                        "active" => Style::default().fg(Color::Blue),
                        _ => Style::default().fg(Color::Red),
                    };
                    Row::new(vec![
                        m.id.chars().take(40).collect::<String>(),
                        m.doc_type.clone().unwrap_or_default(),
                        m.year.clone().unwrap_or_default(),
                        m.status.clone(),
                        progress,
                    ])
                    .style(status_style)
                })
                .collect();

            let table = Table::new(
                rows,
                [
                    Constraint::Percentage(35),
                    Constraint::Percentage(15),
                    Constraint::Percentage(10),
                    Constraint::Percentage(15),
                    Constraint::Percentage(15),
                ],
            )
            .header(header)
            .block(Block::default().title("Recent Manifests").borders(Borders::ALL));
            f.render_widget(table, chunks[3]);

            // Footer
            let footer = Paragraph::new(Line::from(vec![
                Span::styled("q", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(" quit  "),
                Span::styled("r", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw(" refresh"),
            ]));
            f.render_widget(footer, chunks[4]);
        })?;

        // Wait for input or timeout
        if wait_for_quit(args.refresh)? {
            return Ok(());
        }
    }
}

/// Wait for 'q' keypress or timeout. Returns true if should quit.
fn wait_for_quit(timeout_secs: u64) -> Result<bool> {
    if event::poll(Duration::from_secs(timeout_secs))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press && (key.code == KeyCode::Char('q') || key.code == KeyCode::Esc) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct DashboardData {
    total_manifests: i64,
    total_archives: i64,
    total_registries: i64,
    completed_downloads: i64,
    pending_downloads: i64,
    failed_downloads: i64,
    total_tags: i64,
    total_ocr: i64,
    disk_usage: u64,
    recent_manifests: Vec<ManifestStatus>,
}

struct ManifestStatus {
    id: String,
    doc_type: Option<String>,
    year: Option<String>,
    status: String,
    completed: i64,
    total: i64,
}

fn load_dashboard_data() -> Result<DashboardData> {
    let db = StateDb::open(&output::db_path())?;
    let stats = db.get_extended_stats()?;

    // Get recent manifests with download progress
    let recent = db.get_recent_manifest_status(20)?;

    // Calculate disk usage
    let disk_usage = calculate_disk_usage();

    Ok(DashboardData {
        total_manifests: stats.base.manifests as i64,
        total_archives: stats.archives as i64,
        total_registries: stats.registries as i64,
        completed_downloads: stats.base.complete as i64,
        pending_downloads: stats.base.pending as i64,
        failed_downloads: stats.base.failed as i64,
        total_tags: stats.base.tags as i64,
        total_ocr: stats.ocr_results as i64,
        disk_usage,
        recent_manifests: recent
            .into_iter()
            .map(|r| ManifestStatus {
                id: r.id,
                doc_type: r.doc_type,
                year: r.year,
                status: r.status,
                completed: r.completed,
                total: r.total,
            })
            .collect(),
    })
}

fn calculate_disk_usage() -> u64 {
    let base = output::base_dir();
    if !base.exists() {
        return 0;
    }
    walkdir(base).unwrap_or(0)
}

fn walkdir(path: std::path::PathBuf) -> Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(&path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += walkdir(entry.path())?;
        }
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
