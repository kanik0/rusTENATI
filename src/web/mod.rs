mod api;
mod assets;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::Router;
use tower_http::services::ServeDir;

use crate::download::state::StateDb;
use crate::output;

/// Shared application state for all handlers.
pub struct AppState {
    pub db: Mutex<StateDb>,
}

/// Start the web server.
pub async fn start_server(
    bind: &str,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let db = StateDb::open(&output::db_path())?;
    let data_dir = output::base_dir();
    let state = Arc::new(AppState {
        db: Mutex::new(db),
    });

    let app = Router::new()
        .nest("/api/v1", api::routes())
        .nest_service("/images", ServeDir::new(&data_dir))
        .fallback(assets::static_handler)
        .with_state(state);

    let addr = format!("{bind}:{port}");
    println!("Rustenati web interface: http://{addr}");

    if open_browser {
        let url = format!("http://{addr}");
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd").args(["/C", "start", &url]).spawn();
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
