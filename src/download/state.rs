use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// SQLite-backed state database for tracking downloads, manifests, sessions, and tags.
pub struct StateDb {
    conn: Connection,
}

impl StateDb {
    /// Open or create the state database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create state db directory: {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open state database: {}", path.display()))?;

        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS manifests (
                id TEXT PRIMARY KEY,
                archive_id TEXT NOT NULL,
                title TEXT,
                total_canvases INTEGER,
                json_cached TEXT,
                fetched_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                manifest_id TEXT NOT NULL,
                config_snapshot TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                FOREIGN KEY (manifest_id) REFERENCES manifests(id)
            );

            CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                manifest_id TEXT NOT NULL,
                canvas_id TEXT NOT NULL,
                canvas_index INTEGER NOT NULL,
                image_url TEXT NOT NULL,
                local_path TEXT,
                sha256 TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                ocr_status TEXT NOT NULL DEFAULT 'none',
                error_message TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(manifest_id, canvas_id),
                FOREIGN KEY (manifest_id) REFERENCES manifests(id)
            );

            CREATE TABLE IF NOT EXISTS ocr_results (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                download_id INTEGER NOT NULL,
                backend TEXT NOT NULL,
                raw_text TEXT,
                structured_json TEXT,
                processed_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (download_id) REFERENCES downloads(id)
            );

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                download_id INTEGER NOT NULL,
                tag_type TEXT NOT NULL,
                value TEXT NOT NULL,
                confidence REAL,
                source TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (download_id) REFERENCES downloads(id)
            );

            CREATE INDEX IF NOT EXISTS idx_downloads_manifest ON downloads(manifest_id);
            CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
            CREATE INDEX IF NOT EXISTS idx_tags_type_value ON tags(tag_type, value);
            CREATE INDEX IF NOT EXISTS idx_tags_download ON tags(download_id);
            CREATE INDEX IF NOT EXISTS idx_ocr_download ON ocr_results(download_id);
            "
        ).context("Failed to initialize database schema")?;

        Ok(())
    }

    /// Insert or update a manifest record.
    pub fn upsert_manifest(
        &self,
        id: &str,
        archive_id: &str,
        title: Option<&str>,
        total_canvases: usize,
        json_cached: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO manifests (id, archive_id, title, total_canvases, json_cached, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, archive_id, title, total_canvases as i64, json_cached],
        )?;
        Ok(())
    }

    /// Create a new download session.
    pub fn create_session(&self, manifest_id: &str, config_snapshot: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (manifest_id, config_snapshot) VALUES (?1, ?2)",
            params![manifest_id, config_snapshot],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a pending download record. Returns the download ID.
    pub fn insert_download(
        &self,
        manifest_id: &str,
        canvas_id: &str,
        canvas_index: usize,
        image_url: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO downloads (manifest_id, canvas_id, canvas_index, image_url, status)
             VALUES (?1, ?2, ?3, ?4, 'pending')",
            params![manifest_id, canvas_id, canvas_index as i64, image_url],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update download status to 'complete' with local path and checksum.
    pub fn mark_complete(
        &self,
        manifest_id: &str,
        canvas_id: &str,
        local_path: &str,
        sha256: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET status = 'complete', local_path = ?1, sha256 = ?2,
             updated_at = datetime('now') WHERE manifest_id = ?3 AND canvas_id = ?4",
            params![local_path, sha256, manifest_id, canvas_id],
        )?;
        Ok(())
    }

    /// Mark a download as failed.
    pub fn mark_failed(
        &self,
        manifest_id: &str,
        canvas_id: &str,
        error: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET status = 'failed', error_message = ?1,
             updated_at = datetime('now') WHERE manifest_id = ?2 AND canvas_id = ?3",
            params![error, manifest_id, canvas_id],
        )?;
        Ok(())
    }

    /// Get pending or failed downloads for a manifest (for resume).
    pub fn get_incomplete_downloads(&self, manifest_id: &str) -> Result<Vec<DownloadRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, canvas_id, canvas_index, image_url, status
             FROM downloads WHERE manifest_id = ?1 AND status IN ('pending', 'failed')
             ORDER BY canvas_index",
        )?;

        let records = stmt
            .query_map(params![manifest_id], |row| {
                Ok(DownloadRecord {
                    id: row.get(0)?,
                    canvas_id: row.get(1)?,
                    canvas_index: row.get::<_, i64>(2)? as usize,
                    image_url: row.get(3)?,
                    status: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Check if a canvas has already been downloaded successfully.
    pub fn is_downloaded(&self, manifest_id: &str, canvas_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1 AND canvas_id = ?2 AND status = 'complete'",
            params![manifest_id, canvas_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get download statistics for a manifest.
    pub fn get_stats(&self, manifest_id: &str) -> Result<DownloadStats> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1",
            params![manifest_id],
            |row| row.get(0),
        )?;
        let complete: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1 AND status = 'complete'",
            params![manifest_id],
            |row| row.get(0),
        )?;
        let failed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1 AND status = 'failed'",
            params![manifest_id],
            |row| row.get(0),
        )?;
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1 AND status = 'pending'",
            params![manifest_id],
            |row| row.get(0),
        )?;

        Ok(DownloadStats {
            total: total as usize,
            complete: complete as usize,
            failed: failed as usize,
            pending: pending as usize,
        })
    }

    /// Insert a tag for a download.
    pub fn insert_tag(
        &self,
        download_id: i64,
        tag_type: &str,
        value: &str,
        confidence: Option<f32>,
        source: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tags (download_id, tag_type, value, confidence, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![download_id, tag_type, value, confidence, source],
        )?;
        Ok(())
    }

    /// Search tags by type and value pattern.
    pub fn search_tags(
        &self,
        tag_type: Option<&str>,
        value_pattern: Option<&str>,
    ) -> Result<Vec<TagRecord>> {
        let mut sql = "SELECT t.id, t.download_id, t.tag_type, t.value, t.confidence, t.source,
                       d.manifest_id, d.canvas_id
                       FROM tags t JOIN downloads d ON t.download_id = d.id WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(tt) = tag_type {
            sql.push_str(&format!(" AND t.tag_type = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(tt.to_string()));
        }
        if let Some(vp) = value_pattern {
            sql.push_str(&format!(" AND t.value LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{vp}%")));
        }

        sql.push_str(" ORDER BY t.tag_type, t.value");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(TagRecord {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    tag_type: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    manifest_id: row.get(6)?,
                    canvas_id: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Update session status.
    pub fn update_session_status(&self, session_id: i64, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?1 WHERE id = ?2",
            params![status, session_id],
        )?;
        Ok(())
    }

    /// Get tags for a specific download ID.
    pub fn get_tags_for_download(&self, download_id: i64) -> Result<Vec<TagRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.download_id, t.tag_type, t.value, t.confidence, t.source,
                    d.manifest_id, d.canvas_id
             FROM tags t JOIN downloads d ON t.download_id = d.id
             WHERE t.download_id = ?1
             ORDER BY t.tag_type, t.value",
        )?;

        let records = stmt
            .query_map(params![download_id], |row| {
                Ok(TagRecord {
                    id: row.get(0)?,
                    download_id: row.get(1)?,
                    tag_type: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    manifest_id: row.get(6)?,
                    canvas_id: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get tag statistics (count per type).
    pub fn get_tag_stats(&self) -> Result<Vec<TagStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag_type, COUNT(*) as cnt, COUNT(DISTINCT download_id) as downloads
             FROM tags GROUP BY tag_type ORDER BY cnt DESC",
        )?;

        let stats = stmt
            .query_map([], |row| {
                Ok(TagStat {
                    tag_type: row.get(0)?,
                    count: row.get(1)?,
                    unique_downloads: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(stats)
    }

    /// Get total tag count.
    pub fn get_total_tag_count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM tags", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// List all sessions with summary info.
    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.started_at, s.manifest_id, s.status,
                    m.title, m.total_canvases,
                    (SELECT COUNT(*) FROM downloads d WHERE d.manifest_id = s.manifest_id AND d.status = 'complete') as completed,
                    (SELECT COUNT(*) FROM downloads d WHERE d.manifest_id = s.manifest_id AND d.status = 'failed') as failed
             FROM sessions s
             LEFT JOIN manifests m ON s.manifest_id = m.id
             ORDER BY s.id DESC",
        )?;

        let records = stmt
            .query_map([], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    manifest_id: row.get(2)?,
                    status: row.get(3)?,
                    title: row.get(4)?,
                    total_canvases: row.get::<_, Option<i64>>(5)?.unwrap_or(0) as usize,
                    completed: row.get::<_, i64>(6)? as usize,
                    failed: row.get::<_, i64>(7)? as usize,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get a specific session.
    pub fn get_session(&self, session_id: i64) -> Result<Option<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.started_at, s.manifest_id, s.status,
                    m.title, m.total_canvases,
                    (SELECT COUNT(*) FROM downloads d WHERE d.manifest_id = s.manifest_id AND d.status = 'complete') as completed,
                    (SELECT COUNT(*) FROM downloads d WHERE d.manifest_id = s.manifest_id AND d.status = 'failed') as failed
             FROM sessions s
             LEFT JOIN manifests m ON s.manifest_id = m.id
             WHERE s.id = ?1",
        )?;

        let mut rows = stmt.query_map(params![session_id], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                started_at: row.get(1)?,
                manifest_id: row.get(2)?,
                status: row.get(3)?,
                title: row.get(4)?,
                total_canvases: row.get::<_, Option<i64>>(5)?.unwrap_or(0) as usize,
                completed: row.get::<_, i64>(6)? as usize,
                failed: row.get::<_, i64>(7)? as usize,
            })
        })?;

        match rows.next() {
            Some(Ok(record)) => Ok(Some(record)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Get overall download stats across all manifests.
    pub fn get_global_stats(&self) -> Result<GlobalStats> {
        let manifests: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM manifests", [], |row| row.get(0),
        )?;
        let sessions: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions", [], |row| row.get(0),
        )?;
        let total_downloads: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads", [], |row| row.get(0),
        )?;
        let complete: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE status = 'complete'", [], |row| row.get(0),
        )?;
        let failed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE status = 'failed'", [], |row| row.get(0),
        )?;
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE status = 'pending'", [], |row| row.get(0),
        )?;
        let tags: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tags", [], |row| row.get(0),
        )?;

        Ok(GlobalStats {
            manifests: manifests as usize,
            sessions: sessions as usize,
            total_downloads: total_downloads as usize,
            complete: complete as usize,
            failed: failed as usize,
            pending: pending as usize,
            tags: tags as usize,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub id: i64,
    pub canvas_id: String,
    pub canvas_index: usize,
    pub image_url: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct DownloadStats {
    pub total: usize,
    pub complete: usize,
    pub failed: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TagRecord {
    pub id: i64,
    pub download_id: i64,
    pub tag_type: String,
    pub value: String,
    pub confidence: Option<f64>,
    pub source: Option<String>,
    pub manifest_id: String,
    pub canvas_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TagStat {
    pub tag_type: String,
    pub count: i64,
    pub unique_downloads: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRecord {
    pub id: i64,
    pub started_at: String,
    pub manifest_id: String,
    pub status: String,
    pub title: Option<String>,
    pub total_canvases: usize,
    pub completed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GlobalStats {
    pub manifests: usize,
    pub sessions: usize,
    pub total_downloads: usize,
    pub complete: usize,
    pub failed: usize,
    pub pending: usize,
    pub tags: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        let db = StateDb::in_memory().unwrap();
        // Schema should be created without errors
        db.upsert_manifest("test-manifest", "archive-1", Some("Test"), 10, None)
            .unwrap();
    }

    #[test]
    fn test_download_lifecycle() {
        let db = StateDb::in_memory().unwrap();
        db.upsert_manifest("m1", "a1", Some("Test"), 2, None).unwrap();

        db.insert_download("m1", "canvas-1", 0, "http://example.com/1.jpg").unwrap();
        db.insert_download("m1", "canvas-2", 1, "http://example.com/2.jpg").unwrap();

        let incomplete = db.get_incomplete_downloads("m1").unwrap();
        assert_eq!(incomplete.len(), 2);

        db.mark_complete("m1", "canvas-1", "/tmp/1.jpg", "abc123").unwrap();

        let incomplete = db.get_incomplete_downloads("m1").unwrap();
        assert_eq!(incomplete.len(), 1);

        assert!(db.is_downloaded("m1", "canvas-1").unwrap());
        assert!(!db.is_downloaded("m1", "canvas-2").unwrap());

        let stats = db.get_stats("m1").unwrap();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.complete, 1);
        assert_eq!(stats.pending, 1);
    }

    #[test]
    fn test_tags() {
        let db = StateDb::in_memory().unwrap();
        db.upsert_manifest("m1", "a1", Some("Test"), 1, None).unwrap();
        let dl_id = db.insert_download("m1", "c1", 0, "http://example.com/1.jpg").unwrap();

        db.insert_tag(dl_id, "surname", "ROSSI", Some(0.95), Some("claude")).unwrap();
        db.insert_tag(dl_id, "name", "MARIO", Some(0.9), Some("claude")).unwrap();
        db.insert_tag(dl_id, "date", "1807-03-15", None, Some("claude")).unwrap();

        let results = db.search_tags(Some("surname"), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "ROSSI");

        let results = db.search_tags(None, Some("MARIO")).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_sessions() {
        let db = StateDb::in_memory().unwrap();
        db.upsert_manifest("m1", "a1", None, 5, None).unwrap();
        let session_id = db.create_session("m1", Some("{}")).unwrap();
        assert!(session_id > 0);
        db.update_session_status(session_id, "complete").unwrap();
    }
}
