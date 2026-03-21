mod api;
mod assets;
pub mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::routing::get;
use tower_http::services::ServeDir;

use crate::download::state::StateDb;
use crate::output;

pub use ws::EventBroadcaster;

/// Simple connection pool for read-heavy web server workloads.
/// With WAL mode, multiple readers can work concurrently.
#[derive(Clone)]
pub struct DbPool {
    inner: Arc<DbPoolInner>,
}

struct DbPoolInner {
    db_path: PathBuf,
    pool: tokio::sync::Mutex<Vec<StateDb>>,
    max_size: usize,
}

impl DbPool {
    pub fn new(db_path: PathBuf, max_size: usize) -> Result<Self> {
        // Pre-create one connection to validate the path
        let initial = StateDb::open(&db_path)?;
        Ok(Self {
            inner: Arc::new(DbPoolInner {
                db_path,
                pool: tokio::sync::Mutex::new(vec![initial]),
                max_size,
            }),
        })
    }

    /// Get a connection from the pool, or create a new one.
    pub async fn get(&self) -> Result<PoolGuard> {
        let mut pool = self.inner.pool.lock().await;
        if let Some(db) = pool.pop() {
            Ok(PoolGuard { db: Some(db), pool: self.clone() })
        } else {
            drop(pool);
            let path = self.inner.db_path.clone();
            let db = tokio::task::spawn_blocking(move || StateDb::open(&path)).await??;
            Ok(PoolGuard { db: Some(db), pool: self.clone() })
        }
    }

    /// Return a connection to the pool.
    async fn return_conn(&self, db: StateDb) {
        let mut pool = self.inner.pool.lock().await;
        if pool.len() < self.inner.max_size {
            pool.push(db);
        }
    }
}

/// RAII guard that returns the connection to the pool on drop.
pub struct PoolGuard {
    db: Option<StateDb>,
    pool: DbPool,
}

impl PoolGuard {
    pub fn db(&self) -> &StateDb {
        self.db.as_ref().unwrap()
    }
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        if let Some(db) = self.db.take() {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                pool.return_conn(db).await;
            });
        }
    }
}

/// Shared application state for all handlers.
pub struct AppState {
    pub pool: DbPool,
    pub broadcaster: Option<EventBroadcaster>,
}

/// Start the web server.
pub async fn start_server(
    bind: &str,
    port: u16,
    open_browser: bool,
) -> Result<()> {
    let pool = DbPool::new(output::db_path(), 8)?;
    let data_dir = output::base_dir();
    let broadcaster = EventBroadcaster::new(256);
    let state = Arc::new(AppState { pool, broadcaster: Some(broadcaster) });

    let app = Router::new()
        .nest("/api/v1", api::routes())
        .route("/api/v1/ws", get(ws::ws_handler))
        .nest_service("/images", ServeDir::new(&data_dir))
        .fallback(assets::static_handler)
        .with_state(state);

    let addr = format!("{bind}:{port}");
    println!("rusTENATI web interface: http://{addr}");

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
