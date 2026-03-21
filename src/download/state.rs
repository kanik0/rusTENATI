use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::models::manifest::{IiifManifest, MetadataEntry};
use crate::models::search::{NameResult, RegistryResult};

/// Extract a 4-digit year from a date string like "1810/01/01" or "1810".
fn extract_year(date: &str) -> Option<String> {
    // Look for a 4-digit year at the start of the string
    let trimmed = date.trim();
    if trimmed.len() >= 4 && trimmed[..4].chars().all(|c| c.is_ascii_digit()) {
        Some(trimmed[..4].to_string())
    } else {
        None
    }
}

/// Parse a context string like "Stato civile napoleonico > Camposano (provincia di Napoli)"
/// into (locality_name, province).
fn parse_context_locality(context: &str) -> (Option<String>, Option<String>) {
    let locality_part = match context.rsplit(" > ").next() {
        Some(s) if !s.is_empty() => s,
        _ => return (None, None),
    };

    if let Some(idx) = locality_part.find("(provincia di ") {
        let name = locality_part[..idx].trim().to_string();
        let prov = locality_part[idx + 14..].trim_end_matches(')').trim().to_string();
        (Some(name), Some(prov))
    } else {
        (Some(locality_part.trim().to_string()), None)
    }
}

/// SQLite-backed state database for tracking downloads, manifests, sessions, tags,
/// search results, persons, and archives.
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

        Self::apply_pragmas(&conn)?;

        let db = Self { conn };
        db.init_schema()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::apply_pragmas(&conn)?;
        let db = Self { conn };
        db.init_schema()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// Apply performance-critical PRAGMAs to a connection.
    fn apply_pragmas(conn: &Connection) -> Result<()> {
        // journal_mode=WAL may fail for in-memory DBs; ignore errors
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        // These are safe for both file and in-memory databases
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.pragma_update(None, "cache_size", -64000_i64);
        let _ = conn.pragma_update(None, "mmap_size", 268435456_i64);
        let _ = conn.pragma_update(None, "temp_store", "MEMORY");
        let _ = conn.pragma_update(None, "journal_size_limit", 67108864_i64);
        let _ = conn.pragma_update(None, "wal_autocheckpoint", 1000_i64);
        let _ = conn.pragma_update(None, "busy_timeout", 5000_i64);
        Ok(())
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

            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "
        ).context("Failed to initialize database schema")?;

        Ok(())
    }

    // ─── Migration System ───────────────────────────────────────────────

    fn get_schema_version(&self) -> Result<i32> {
        let version: i32 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 1) FROM schema_version",
            [],
            |row| row.get(0),
        )?;
        Ok(version)
    }

    fn run_migrations(&self) -> Result<()> {
        let current = self.get_schema_version()?;

        if current < 2 {
            self.migrate_v2()?;
        }
        if current < 3 {
            self.migrate_v3()?;
        }

        Ok(())
    }

    fn migrate_v2(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN TRANSACTION;")?;

        let result = (|| -> Result<()> {
            // --- New tables ---

            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS archives (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    slug TEXT NOT NULL UNIQUE,
                    url TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_archives_name ON archives(name);

                CREATE TABLE IF NOT EXISTS localities (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    province TEXT,
                    normalized_name TEXT NOT NULL,
                    UNIQUE(name, province)
                );
                CREATE INDEX IF NOT EXISTS idx_localities_normalized ON localities(normalized_name);

                CREATE TABLE IF NOT EXISTS search_queries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    query_type TEXT NOT NULL,
                    params_json TEXT NOT NULL,
                    total_results INTEGER,
                    pages_fetched INTEGER,
                    executed_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE TABLE IF NOT EXISTS registry_results (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    query_id INTEGER NOT NULL REFERENCES search_queries(id),
                    ark_url TEXT NOT NULL,
                    year TEXT,
                    doc_type TEXT,
                    signature TEXT,
                    context TEXT,
                    archive_name TEXT,
                    archive_url TEXT,
                    manifest_id TEXT REFERENCES manifests(id),
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(query_id, ark_url)
                );
                CREATE INDEX IF NOT EXISTS idx_registry_results_ark ON registry_results(ark_url);
                CREATE INDEX IF NOT EXISTS idx_registry_results_year ON registry_results(year);
                CREATE INDEX IF NOT EXISTS idx_registry_results_doc_type ON registry_results(doc_type);
                CREATE INDEX IF NOT EXISTS idx_registry_results_query ON registry_results(query_id);

                CREATE TABLE IF NOT EXISTS persons (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    surname TEXT,
                    given_name TEXT,
                    detail_url TEXT UNIQUE,
                    birth_info TEXT,
                    death_info TEXT,
                    birth_place TEXT,
                    birth_year INTEGER,
                    death_place TEXT,
                    death_year INTEGER,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_persons_surname ON persons(surname);
                CREATE INDEX IF NOT EXISTS idx_persons_name ON persons(name);
                CREATE INDEX IF NOT EXISTS idx_persons_birth_year ON persons(birth_year);

                CREATE TABLE IF NOT EXISTS person_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    person_id INTEGER NOT NULL REFERENCES persons(id),
                    record_type TEXT,
                    date TEXT,
                    ark_url TEXT,
                    manifest_id TEXT REFERENCES manifests(id),
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(person_id, ark_url)
                );
                CREATE INDEX IF NOT EXISTS idx_person_records_person ON person_records(person_id);
                CREATE INDEX IF NOT EXISTS idx_person_records_ark ON person_records(ark_url);

                CREATE TABLE IF NOT EXISTS person_search_results (
                    query_id INTEGER NOT NULL REFERENCES search_queries(id),
                    person_id INTEGER NOT NULL REFERENCES persons(id),
                    result_index INTEGER NOT NULL,
                    PRIMARY KEY (query_id, person_id)
                );

                CREATE TABLE IF NOT EXISTS manifest_metadata (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    manifest_id TEXT NOT NULL REFERENCES manifests(id),
                    label TEXT NOT NULL,
                    value TEXT NOT NULL,
                    UNIQUE(manifest_id, label)
                );
                CREATE INDEX IF NOT EXISTS idx_manifest_metadata_manifest ON manifest_metadata(manifest_id);
                CREATE INDEX IF NOT EXISTS idx_manifest_metadata_label ON manifest_metadata(label);
                "
            )?;

            // --- ALTER TABLE: add columns to manifests ---
            let manifest_columns = [
                ("ark_url", "TEXT"),
                ("doc_type", "TEXT"),
                ("archival_context", "TEXT"),
                ("archive_db_id", "INTEGER REFERENCES archives(id)"),
                ("locality_id", "INTEGER REFERENCES localities(id)"),
                ("signature", "TEXT"),
                ("date_from", "TEXT"),
                ("date_to", "TEXT"),
                ("license", "TEXT"),
                ("language", "TEXT"),
                ("iiif_version", "TEXT"),
                ("year", "TEXT"),
            ];

            for (col, typ) in &manifest_columns {
                // ALTER TABLE ADD COLUMN is idempotent-safe: if column exists, it errors,
                // but we ignore that error.
                let sql = format!("ALTER TABLE manifests ADD COLUMN {col} {typ}");
                let _ = self.conn.execute_batch(&sql);
            }

            // --- ALTER TABLE: add columns to downloads ---
            let download_columns = [
                ("canvas_label", "TEXT"),
                ("width", "INTEGER"),
                ("height", "INTEGER"),
            ];

            for (col, typ) in &download_columns {
                let sql = format!("ALTER TABLE downloads ADD COLUMN {col} {typ}");
                let _ = self.conn.execute_batch(&sql);
            }

            // --- Indexes on new manifest columns ---
            self.conn.execute_batch(
                "
                CREATE INDEX IF NOT EXISTS idx_manifests_doc_type ON manifests(doc_type);
                CREATE INDEX IF NOT EXISTS idx_manifests_archive ON manifests(archive_db_id);
                CREATE INDEX IF NOT EXISTS idx_manifests_locality ON manifests(locality_id);
                CREATE INDEX IF NOT EXISTS idx_manifests_year ON manifests(year);
                CREATE INDEX IF NOT EXISTS idx_manifests_ark ON manifests(ark_url);
                "
            )?;

            // --- FTS5 for OCR full-text search ---
            self.conn.execute_batch(
                "
                CREATE VIRTUAL TABLE IF NOT EXISTS ocr_fulltext USING fts5(
                    text,
                    content='ocr_results',
                    content_rowid='id'
                );

                CREATE TRIGGER IF NOT EXISTS ocr_results_ai AFTER INSERT ON ocr_results BEGIN
                    INSERT INTO ocr_fulltext(rowid, text) VALUES (new.id, new.raw_text);
                END;

                CREATE TRIGGER IF NOT EXISTS ocr_results_ad AFTER DELETE ON ocr_results BEGIN
                    INSERT INTO ocr_fulltext(ocr_fulltext, rowid, text) VALUES('delete', old.id, old.raw_text);
                END;

                CREATE TRIGGER IF NOT EXISTS ocr_results_au AFTER UPDATE ON ocr_results BEGIN
                    INSERT INTO ocr_fulltext(ocr_fulltext, rowid, text) VALUES('delete', old.id, old.raw_text);
                    INSERT INTO ocr_fulltext(rowid, text) VALUES (new.id, new.raw_text);
                END;
                "
            )?;

            // --- Backfill FTS from existing ocr_results ---
            self.conn.execute_batch(
                "INSERT OR IGNORE INTO ocr_fulltext(rowid, text)
                 SELECT id, raw_text FROM ocr_results WHERE raw_text IS NOT NULL;"
            )?;

            // --- Record migration version ---
            self.conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                params![2],
            )?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn migrate_v3(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN TRANSACTION;")?;

        let result = (|| -> Result<()> {
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS registries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ark_url TEXT NOT NULL UNIQUE,
                    year TEXT,
                    doc_type TEXT,
                    signature TEXT,
                    context TEXT,
                    archive_name TEXT,
                    archive_url TEXT,
                    archive_db_id INTEGER REFERENCES archives(id),
                    locality_name TEXT,
                    province TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );

                CREATE INDEX IF NOT EXISTS idx_registries_ark ON registries(ark_url);
                CREATE INDEX IF NOT EXISTS idx_registries_year ON registries(year);
                CREATE INDEX IF NOT EXISTS idx_registries_doc_type ON registries(doc_type);
                CREATE INDEX IF NOT EXISTS idx_registries_archive ON registries(archive_db_id);
                "
            )?;

            self.conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                params![3],
            )?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    // ─── Manifest Methods (original + expanded) ─────────────────────────

    /// Insert or update a manifest record (backward-compatible).
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

    /// Insert or update a manifest with all expanded metadata fields.
    pub fn upsert_manifest_full(&self, insert: &ManifestInsert<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO manifests (id, archive_id, title, total_canvases, json_cached, fetched_at,
                ark_url, doc_type, archival_context, archive_db_id, locality_id,
                signature, date_from, date_to, license, language, iiif_version, year)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'),
                ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                archive_id = excluded.archive_id,
                title = excluded.title,
                total_canvases = excluded.total_canvases,
                json_cached = COALESCE(excluded.json_cached, manifests.json_cached),
                fetched_at = datetime('now'),
                ark_url = COALESCE(excluded.ark_url, manifests.ark_url),
                doc_type = COALESCE(excluded.doc_type, manifests.doc_type),
                archival_context = COALESCE(excluded.archival_context, manifests.archival_context),
                archive_db_id = COALESCE(excluded.archive_db_id, manifests.archive_db_id),
                locality_id = COALESCE(excluded.locality_id, manifests.locality_id),
                signature = COALESCE(excluded.signature, manifests.signature),
                date_from = COALESCE(excluded.date_from, manifests.date_from),
                date_to = COALESCE(excluded.date_to, manifests.date_to),
                license = COALESCE(excluded.license, manifests.license),
                language = COALESCE(excluded.language, manifests.language),
                iiif_version = COALESCE(excluded.iiif_version, manifests.iiif_version),
                year = COALESCE(excluded.year, manifests.year)",
            params![
                insert.id, insert.archive_id, insert.title,
                insert.total_canvases.map(|v| v as i64), insert.json_cached,
                insert.ark_url, insert.doc_type, insert.archival_context,
                insert.archive_db_id, insert.locality_id,
                insert.signature, insert.date_from, insert.date_to,
                insert.license, insert.language, insert.iiif_version, insert.year,
            ],
        )?;
        Ok(())
    }

    /// Build a ManifestInsert from an IiifManifest and persist it along with all metadata.
    pub fn store_manifest_from_iiif(
        &self,
        manifest: &IiifManifest,
        ark_url: Option<&str>,
    ) -> Result<()> {
        let archive_context = manifest.archival_context();
        let archive_name = manifest.get_metadata("Conservato da");

        // Upsert archive if present
        let archive_db_id = if let Some(name) = archive_name {
            let slug = name.to_lowercase()
                .replace(' ', "-")
                .replace('\'', "");
            Some(self.upsert_archive(name, &slug, None)?)
        } else {
            None
        };

        // Extract year from date range
        let date_from = manifest.get_metadata("Estremo remoto");
        let date_to = manifest.get_metadata("Estremo recente");
        let year = date_from.map(|d| d.to_string())
            .or_else(|| manifest.get_metadata("Datazione").map(|d| d.to_string()));

        let insert = ManifestInsert {
            id: &manifest.id,
            archive_id: archive_context.unwrap_or("unknown"),
            title: Some(manifest.title()),
            total_canvases: Some(manifest.canvases.len()),
            json_cached: None,
            ark_url,
            doc_type: manifest.doc_type(),
            archival_context: archive_context,
            archive_db_id,
            locality_id: None,
            signature: manifest.get_metadata("Segnatura attuale"),
            date_from,
            date_to,
            license: manifest.get_metadata("Licenza"),
            language: manifest.get_metadata("Lingua"),
            iiif_version: Some(&manifest.version.to_string()),
            year: year.as_deref(),
        };

        self.upsert_manifest_full(&insert)?;
        self.store_manifest_metadata(&manifest.id, &manifest.metadata)?;

        Ok(())
    }

    /// Store all raw metadata key-value pairs for a manifest.
    pub fn store_manifest_metadata(&self, manifest_id: &str, metadata: &[MetadataEntry]) -> Result<()> {
        for entry in metadata {
            self.conn.execute(
                "INSERT INTO manifest_metadata (manifest_id, label, value)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(manifest_id, label) DO UPDATE SET value = excluded.value",
                params![manifest_id, entry.label, entry.value],
            )?;
        }
        Ok(())
    }

    /// Get all raw metadata for a manifest.
    pub fn get_manifest_metadata(&self, manifest_id: &str) -> Result<Vec<MetadataEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT label, value FROM manifest_metadata WHERE manifest_id = ?1 ORDER BY label",
        )?;
        let records = stmt
            .query_map(params![manifest_id], |row| {
                Ok(MetadataEntry {
                    label: row.get(0)?,
                    value: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Search manifests by various criteria.
    pub fn search_manifests(
        &self,
        doc_type: Option<&str>,
        year: Option<&str>,
        archive_name: Option<&str>,
        locality: Option<&str>,
    ) -> Result<Vec<ManifestRecord>> {
        let mut sql = "SELECT m.id, m.archive_id, m.title, m.total_canvases, m.doc_type,
                       m.archival_context, m.signature, m.date_from, m.date_to,
                       m.iiif_version, m.year, m.ark_url,
                       a.name as archive_name
                       FROM manifests m
                       LEFT JOIN archives a ON m.archive_db_id = a.id
                       WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(dt) = doc_type {
            sql.push_str(&format!(" AND m.doc_type = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(dt.to_string()));
        }
        if let Some(y) = year {
            sql.push_str(&format!(" AND m.year LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{y}%")));
        }
        if let Some(an) = archive_name {
            sql.push_str(&format!(" AND a.name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{an}%")));
        }
        if let Some(loc) = locality {
            sql.push_str(&format!(" AND m.archival_context LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{loc}%")));
        }

        sql.push_str(" ORDER BY m.year, m.doc_type");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(ManifestRecord {
                    id: row.get(0)?,
                    archive_id: row.get(1)?,
                    title: row.get(2)?,
                    total_canvases: row.get::<_, Option<i64>>(3)?.map(|v| v as usize),
                    doc_type: row.get(4)?,
                    archival_context: row.get(5)?,
                    signature: row.get(6)?,
                    date_from: row.get(7)?,
                    date_to: row.get(8)?,
                    iiif_version: row.get(9)?,
                    year: row.get(10)?,
                    ark_url: row.get(11)?,
                    archive_name: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    // ─── Archive Methods ────────────────────────────────────────────────

    /// Insert or find an existing archive. Returns the archive ID.
    pub fn upsert_archive(&self, name: &str, slug: &str, url: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO archives (name, slug, url)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(slug) DO UPDATE SET
                name = excluded.name,
                url = COALESCE(excluded.url, archives.url),
                updated_at = datetime('now')",
            params![name, slug, url],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM archives WHERE slug = ?1",
            params![slug],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// List all known archives.
    pub fn list_archives(&self) -> Result<Vec<ArchiveRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, slug, url FROM archives ORDER BY name",
        )?;
        let records = stmt
            .query_map([], |row| {
                Ok(ArchiveRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    slug: row.get(2)?,
                    url: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    // ─── Locality Methods ───────────────────────────────────────────────

    /// Insert or find an existing locality. Returns the locality ID.
    pub fn upsert_locality(&self, name: &str, province: Option<&str>) -> Result<i64> {
        let normalized = name.to_lowercase();
        self.conn.execute(
            "INSERT INTO localities (name, province, normalized_name)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(name, province) DO NOTHING",
            params![name, province, normalized],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM localities WHERE name = ?1 AND province IS ?2",
            params![name, province],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// Search localities by name pattern.
    pub fn search_localities(&self, pattern: &str) -> Result<Vec<LocalityRecord>> {
        let normalized = pattern.to_lowercase();
        let mut stmt = self.conn.prepare(
            "SELECT id, name, province FROM localities
             WHERE normalized_name LIKE ?1 ORDER BY name",
        )?;
        let records = stmt
            .query_map(params![format!("%{normalized}%")], |row| {
                Ok(LocalityRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    province: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    // ─── Search Query Caching ───────────────────────────────────────────

    /// Record a search query. Returns the query ID.
    pub fn insert_search_query(
        &self,
        query_type: &str,
        params_json: &str,
        total_results: Option<u32>,
        pages_fetched: Option<u32>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO search_queries (query_type, params_json, total_results, pages_fetched)
             VALUES (?1, ?2, ?3, ?4)",
            params![query_type, params_json, total_results, pages_fetched],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a registry search result.
    pub fn insert_registry_result(&self, query_id: i64, result: &RegistryResult) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO registry_results
                (query_id, ark_url, year, doc_type, signature, context, archive_name, archive_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                query_id, result.ark_url, result.year, result.doc_type,
                result.signature, result.context, result.archive, result.archive_url,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Link a registry result to a manifest (after the manifest is downloaded).
    pub fn link_registry_to_manifest(&self, ark_url: &str, manifest_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE registry_results SET manifest_id = ?1 WHERE ark_url = ?2",
            params![manifest_id, ark_url],
        )?;
        Ok(())
    }

    // ─── Registry Catalog Methods ────────────────────────────────────────

    /// Upsert a single registry into the persistent catalog.
    pub fn upsert_registry(&self, result: &RegistryResult) -> Result<i64> {
        let (locality_name, province) = parse_context_locality(&result.context);

        // Upsert archive if we have a name
        let archive_db_id = if !result.archive.is_empty() {
            let slug = result.archive.to_lowercase()
                .replace(' ', "-")
                .replace('\'', "");
            Some(self.upsert_archive(&result.archive, &slug, result.archive_url.as_deref())?)
        } else {
            None
        };

        self.conn.execute(
            "INSERT INTO registries (ark_url, year, doc_type, signature, context,
                archive_name, archive_url, archive_db_id, locality_name, province)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(ark_url) DO UPDATE SET
                year = excluded.year,
                doc_type = excluded.doc_type,
                signature = excluded.signature,
                context = excluded.context,
                archive_name = excluded.archive_name,
                archive_url = excluded.archive_url,
                archive_db_id = excluded.archive_db_id,
                locality_name = excluded.locality_name,
                province = excluded.province,
                updated_at = datetime('now')",
            params![
                result.ark_url, result.year, result.doc_type, result.signature,
                result.context, result.archive, result.archive_url,
                archive_db_id, locality_name, province,
            ],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM registries WHERE ark_url = ?1",
            params![result.ark_url],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// Upsert a batch of registries in a single transaction.
    pub fn upsert_registries_batch(&self, results: &[RegistryResult]) -> Result<usize> {
        self.conn.execute_batch("BEGIN TRANSACTION;")?;
        let mut count = 0;
        let result = (|| -> Result<usize> {
            for r in results {
                self.upsert_registry(r)?;
                count += 1;
            }
            Ok(count)
        })();
        match result {
            Ok(n) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(n)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Create a registry catalog entry from an IIIF manifest's metadata.
    /// Used when downloading a single manifest directly (not via search).
    pub fn upsert_registry_from_manifest(
        &self,
        manifest: &IiifManifest,
        ark_url: Option<&str>,
    ) -> Result<()> {
        let Some(ark) = ark_url else { return Ok(()) };

        let archive_name = manifest.get_metadata("Conservato da").unwrap_or_default();
        let context = manifest.archival_context().unwrap_or_default();
        let doc_type = manifest.doc_type().unwrap_or_default();
        let signature = manifest.get_metadata("Segnatura attuale").unwrap_or_default();
        let date_from = manifest.get_metadata("Estremo remoto")
            .or_else(|| manifest.get_metadata("Datazione"))
            .unwrap_or_default();
        // Extract just the 4-digit year from dates like "1810/01/01"
        let year = extract_year(date_from);

        let result = RegistryResult {
            ark_url: ark.to_string(),
            year: year.unwrap_or_else(|| date_from.to_string()),
            doc_type: doc_type.to_string(),
            signature: signature.to_string(),
            context: context.to_string(),
            archive: archive_name.to_string(),
            archive_url: None,
        };

        self.upsert_registry(&result)?;
        Ok(())
    }

    /// Search the persistent registries catalog with pagination and optional has_images filter.
    pub fn search_registries_catalog(
        &self,
        doc_type: Option<&str>,
        year: Option<&str>,
        archive_name: Option<&str>,
        locality: Option<&str>,
        has_images: Option<bool>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<RegistryCatalogRecord>, usize)> {
        let mut where_clause = "WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(dt) = doc_type {
            where_clause.push_str(&format!(" AND r.doc_type = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(dt.to_string()));
        }
        if let Some(y) = year {
            where_clause.push_str(&format!(" AND r.year LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{y}%")));
        }
        if let Some(an) = archive_name {
            where_clause.push_str(&format!(" AND r.archive_name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{an}%")));
        }
        if let Some(loc) = locality {
            where_clause.push_str(&format!(" AND (r.locality_name LIKE ?{0} OR r.context LIKE ?{0})", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{loc}%")));
        }
        if let Some(true) = has_images {
            where_clause.push_str(
                " AND r.ark_url IN (SELECT DISTINCT m.ark_url FROM manifests m \
                 JOIN downloads d ON d.manifest_id = m.id WHERE d.status = 'complete')"
            );
        }
        if let Some(false) = has_images {
            where_clause.push_str(
                " AND r.ark_url NOT IN (SELECT DISTINCT m.ark_url FROM manifests m \
                 JOIN downloads d ON d.manifest_id = m.id WHERE d.status = 'complete')"
            );
        }

        // Count query
        let count_sql = format!("SELECT COUNT(*) FROM registries r {where_clause}");
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let total: i64 = self.conn.query_row(&count_sql, params_refs.as_slice(), |row| row.get(0))?;

        // Data query
        let data_sql = format!(
            "SELECT r.id, r.ark_url, r.year, r.doc_type, r.signature, r.context,
                    r.archive_name, r.locality_name, r.province, r.updated_at,
                    EXISTS (SELECT 1 FROM manifests m JOIN downloads d ON d.manifest_id = m.id
                            WHERE m.ark_url = r.ark_url AND d.status = 'complete') as has_images
             FROM registries r
             {where_clause}
             ORDER BY r.year, r.doc_type
             LIMIT ?{} OFFSET ?{}",
            params_vec.len() + 1,
            params_vec.len() + 2,
        );
        params_vec.push(Box::new(limit as i64));
        params_vec.push(Box::new(offset as i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&data_sql)?;
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(RegistryCatalogRecord {
                    id: row.get(0)?,
                    ark_url: row.get(1)?,
                    year: row.get(2)?,
                    doc_type: row.get(3)?,
                    signature: row.get(4)?,
                    context: row.get(5)?,
                    archive_name: row.get(6)?,
                    locality_name: row.get(7)?,
                    province: row.get(8)?,
                    updated_at: row.get(9)?,
                    has_images: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((records, total as usize))
    }

    /// Get facets (distinct values) from the registries catalog.
    pub fn get_registry_facets(&self) -> Result<RegistryFacets> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT doc_type FROM registries WHERE doc_type IS NOT NULL ORDER BY doc_type",
        )?;
        let doc_types = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT year FROM registries WHERE year IS NOT NULL ORDER BY year",
        )?;
        let years = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT archive_name FROM registries WHERE archive_name IS NOT NULL ORDER BY archive_name",
        )?;
        let archives = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;

        Ok(RegistryFacets { doc_types, years, archives })
    }

    /// Search cached registry results locally.
    pub fn search_registry_results(
        &self,
        doc_type: Option<&str>,
        year: Option<&str>,
        archive_name: Option<&str>,
        locality: Option<&str>,
    ) -> Result<Vec<RegistryResultRecord>> {
        let mut sql = "SELECT id, ark_url, year, doc_type, signature, context,
                       archive_name, archive_url, manifest_id
                       FROM registry_results WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(dt) = doc_type {
            sql.push_str(&format!(" AND doc_type = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(dt.to_string()));
        }
        if let Some(y) = year {
            sql.push_str(&format!(" AND year = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(y.to_string()));
        }
        if let Some(an) = archive_name {
            sql.push_str(&format!(" AND archive_name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{an}%")));
        }
        if let Some(loc) = locality {
            sql.push_str(&format!(" AND context LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{loc}%")));
        }

        sql.push_str(" ORDER BY year, doc_type");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(RegistryResultRecord {
                    id: row.get(0)?,
                    ark_url: row.get(1)?,
                    year: row.get(2)?,
                    doc_type: row.get(3)?,
                    signature: row.get(4)?,
                    context: row.get(5)?,
                    archive_name: row.get(6)?,
                    archive_url: row.get(7)?,
                    manifest_id: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    // ─── Person Methods ─────────────────────────────────────────────────

    /// Upsert a person from name search results. Returns the person ID.
    pub fn upsert_person(&self, result: &NameResult) -> Result<i64> {
        // Split name into surname/given_name (heuristic: first word is surname)
        let parts: Vec<&str> = result.name.splitn(2, ' ').collect();
        let (surname, given_name) = if parts.len() == 2 {
            (Some(parts[0]), Some(parts[1]))
        } else {
            (Some(result.name.as_str()), None)
        };

        self.conn.execute(
            "INSERT INTO persons (name, surname, given_name, detail_url, birth_info, death_info)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(detail_url) DO UPDATE SET
                name = excluded.name,
                surname = excluded.surname,
                given_name = excluded.given_name,
                birth_info = COALESCE(excluded.birth_info, persons.birth_info),
                death_info = COALESCE(excluded.death_info, persons.death_info),
                updated_at = datetime('now')",
            params![
                result.name, surname, given_name,
                result.detail_url, result.birth_info, result.death_info,
            ],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM persons WHERE detail_url = ?1",
            params![result.detail_url],
            |row| row.get(0),
        )?;

        // Insert linked records
        for rec in &result.records {
            self.conn.execute(
                "INSERT OR IGNORE INTO person_records (person_id, record_type, date, ark_url)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, rec.record_type, rec.date, rec.ark_url],
            )?;
        }

        Ok(id)
    }

    /// Record that a person appeared in a search query.
    pub fn insert_person_search_result(
        &self,
        query_id: i64,
        person_id: i64,
        result_index: i32,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO person_search_results (query_id, person_id, result_index)
             VALUES (?1, ?2, ?3)",
            params![query_id, person_id, result_index],
        )?;
        Ok(())
    }

    /// Search persons by surname and/or name.
    pub fn search_persons(
        &self,
        surname: Option<&str>,
        given_name: Option<&str>,
    ) -> Result<Vec<PersonRecord>> {
        let mut sql = "SELECT id, name, surname, given_name, detail_url,
                       birth_info, death_info
                       FROM persons WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = surname {
            sql.push_str(&format!(" AND surname LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{s}%")));
        }
        if let Some(n) = given_name {
            sql.push_str(&format!(" AND given_name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{n}%")));
        }

        sql.push_str(" ORDER BY surname, given_name");

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(PersonRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    surname: row.get(2)?,
                    given_name: row.get(3)?,
                    detail_url: row.get(4)?,
                    birth_info: row.get(5)?,
                    death_info: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get all records linked to a person.
    pub fn get_person_records(&self, person_id: i64) -> Result<Vec<PersonRecordEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, record_type, date, ark_url, manifest_id
             FROM person_records WHERE person_id = ?1 ORDER BY date",
        )?;
        let records = stmt
            .query_map(params![person_id], |row| {
                Ok(PersonRecordEntry {
                    id: row.get(0)?,
                    record_type: row.get(1)?,
                    date: row.get(2)?,
                    ark_url: row.get(3)?,
                    manifest_id: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    // ─── OCR Full-Text Search ───────────────────────────────────────────

    /// Full-text search on OCR results. Returns matching records with context.
    pub fn search_ocr_text(&self, query: &str, limit: usize) -> Result<Vec<OcrSearchResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT o.id, o.download_id, o.backend, snippet(ocr_fulltext, 0, '>>>', '<<<', '...', 40) as snippet,
                    d.manifest_id, d.canvas_id, d.canvas_index, d.canvas_label,
                    m.title as manifest_title
             FROM ocr_fulltext ft
             JOIN ocr_results o ON ft.rowid = o.id
             JOIN downloads d ON o.download_id = d.id
             JOIN manifests m ON d.manifest_id = m.id
             WHERE ocr_fulltext MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let records = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(OcrSearchResult {
                    ocr_id: row.get(0)?,
                    download_id: row.get(1)?,
                    backend: row.get(2)?,
                    snippet: row.get(3)?,
                    manifest_id: row.get(4)?,
                    canvas_id: row.get(5)?,
                    canvas_index: row.get::<_, i64>(6)? as usize,
                    canvas_label: row.get(7)?,
                    manifest_title: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Rebuild the FTS index from existing OCR results.
    pub fn rebuild_fts_index(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM ocr_fulltext;
             INSERT INTO ocr_fulltext(rowid, text)
             SELECT id, raw_text FROM ocr_results WHERE raw_text IS NOT NULL;"
        )?;
        Ok(())
    }

    // ─── Download Methods (original) ────────────────────────────────────

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

    /// Insert a pending download record with canvas metadata.
    pub fn insert_download_full(
        &self,
        manifest_id: &str,
        canvas_id: &str,
        canvas_index: usize,
        image_url: &str,
        canvas_label: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<i64> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO downloads (manifest_id, canvas_id, canvas_index, image_url, status,
                canvas_label, width, height)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)
             ON CONFLICT(manifest_id, canvas_id) DO UPDATE SET
                canvas_label = COALESCE(excluded.canvas_label, downloads.canvas_label),
                width = COALESCE(excluded.width, downloads.width),
                height = COALESCE(excluded.height, downloads.height)",
        )?;
        stmt.execute(params![
            manifest_id, canvas_id, canvas_index as i64, image_url,
            canvas_label, width, height,
        ])?;
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

    /// Flush a batch of download results in a single transaction.
    /// Each entry is (manifest_id, canvas_id, local_path, sha256, error).
    pub fn flush_download_results(&self, results: &[DownloadResultBatch]) -> Result<()> {
        self.conn.execute_batch("BEGIN TRANSACTION;")?;
        let result = (|| -> Result<()> {
            for r in results {
                match &r.error {
                    Some(err) => {
                        self.conn.execute(
                            "UPDATE downloads SET status = 'failed', error_message = ?1,
                             updated_at = datetime('now') WHERE manifest_id = ?2 AND canvas_id = ?3",
                            params![err, r.manifest_id, r.canvas_id],
                        )?;
                    }
                    None => {
                        self.conn.execute(
                            "UPDATE downloads SET status = 'complete', local_path = ?1, sha256 = ?2,
                             updated_at = datetime('now') WHERE manifest_id = ?3 AND canvas_id = ?4",
                            params![r.local_path, r.sha256, r.manifest_id, r.canvas_id],
                        )?;
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Reset failed downloads back to pending for retry.
    pub fn reset_failed_to_pending(&self, manifest_id: &str) -> Result<usize> {
        let count = self.conn.execute(
            "UPDATE downloads SET status = 'pending', error_message = NULL,
             updated_at = datetime('now') WHERE manifest_id = ?1 AND status = 'failed'",
            params![manifest_id],
        )?;
        Ok(count)
    }

    /// Reset a single failed download to pending.
    pub fn reset_failed_to_pending_single(&self, manifest_id: &str, canvas_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET status = 'pending', error_message = NULL,
             updated_at = datetime('now') WHERE manifest_id = ?1 AND canvas_id = ?2",
            params![manifest_id, canvas_id],
        )?;
        Ok(())
    }

    /// Get all completed downloads, optionally filtered by manifest.
    pub fn get_completed_downloads(&self, manifest_id: Option<&str>) -> Result<Vec<CompletedDownload>> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match manifest_id {
            Some(mid) => (
                "SELECT manifest_id, canvas_id, local_path, COALESCE(sha256, '') \
                 FROM downloads WHERE status = 'complete' AND local_path IS NOT NULL \
                 AND manifest_id = ?1 ORDER BY manifest_id, canvas_index".to_string(),
                vec![Box::new(mid.to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT manifest_id, canvas_id, local_path, COALESCE(sha256, '') \
                 FROM downloads WHERE status = 'complete' AND local_path IS NOT NULL \
                 ORDER BY manifest_id, canvas_index".to_string(),
                vec![],
            ),
        };
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(CompletedDownload {
                    manifest_id: row.get(0)?,
                    canvas_id: row.get(1)?,
                    local_path: row.get(2)?,
                    sha256: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
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
        let mut stmt = self.conn.prepare_cached(
            "SELECT COUNT(*) FROM downloads WHERE manifest_id = ?1 AND canvas_id = ?2 AND status = 'complete'",
        )?;
        let count: i64 = stmt.query_row(params![manifest_id, canvas_id], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Get all completed canvas IDs for a manifest in a single query.
    /// Much more efficient than calling is_downloaded() per canvas.
    pub fn get_downloaded_canvas_ids(&self, manifest_id: &str) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT canvas_id FROM downloads WHERE manifest_id = ?1 AND status = 'complete'",
        )?;
        let ids = stmt
            .query_map(params![manifest_id], |row| row.get::<_, String>(0))?
            .collect::<Result<std::collections::HashSet<_>, _>>()?;
        Ok(ids)
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

    // ─── Tag Methods (original) ─────────────────────────────────────────

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

    // ─── Session & Stats Methods (original + expanded) ──────────────────

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

    /// Get expanded stats including archives, persons, OCR, and search data.
    pub fn get_extended_stats(&self) -> Result<ExtendedStats> {
        let base = self.get_global_stats()?;
        let archives: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM archives", [], |row| row.get(0),
        )?;
        let persons: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM persons", [], |row| row.get(0),
        )?;
        let search_queries: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM search_queries", [], |row| row.get(0),
        )?;
        let registry_results: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM registry_results", [], |row| row.get(0),
        )?;
        let ocr_results: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM ocr_results", [], |row| row.get(0),
        )?;
        let localities: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM localities", [], |row| row.get(0),
        )?;
        let registries: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM registries", [], |row| row.get(0),
        )?;

        Ok(ExtendedStats {
            base,
            archives: archives as usize,
            localities: localities as usize,
            persons: persons as usize,
            search_queries: search_queries as usize,
            registry_results: registry_results as usize,
            registries: registries as usize,
            ocr_results: ocr_results as usize,
        })
    }

    // ─── Web API Methods ─────────────────────────────────────────────────

    /// Get a single manifest by ID.
    pub fn get_manifest_by_id(&self, id: &str) -> Result<Option<ManifestRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.archive_id, m.title, m.total_canvases, m.doc_type,
                    m.archival_context, m.signature, m.date_from, m.date_to,
                    m.iiif_version, m.year, m.ark_url,
                    a.name as archive_name
             FROM manifests m
             LEFT JOIN archives a ON m.archive_db_id = a.id
             WHERE m.id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(ManifestRecord {
                id: row.get(0)?,
                archive_id: row.get(1)?,
                title: row.get(2)?,
                total_canvases: row.get::<_, Option<i64>>(3)?.map(|v| v as usize),
                doc_type: row.get(4)?,
                archival_context: row.get(5)?,
                signature: row.get(6)?,
                date_from: row.get(7)?,
                date_to: row.get(8)?,
                iiif_version: row.get(9)?,
                year: row.get(10)?,
                ark_url: row.get(11)?,
                archive_name: row.get(12)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get all downloads for a manifest (all statuses), with full metadata.
    pub fn get_all_downloads_for_manifest(&self, manifest_id: &str) -> Result<Vec<FullDownloadRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, manifest_id, canvas_id, canvas_index, canvas_label,
                    image_url, local_path, status, ocr_status, width, height
             FROM downloads WHERE manifest_id = ?1
             ORDER BY canvas_index",
        )?;
        let records = stmt
            .query_map(params![manifest_id], |row| {
                Ok(FullDownloadRecord {
                    id: row.get(0)?,
                    manifest_id: row.get(1)?,
                    canvas_id: row.get(2)?,
                    canvas_index: row.get::<_, i64>(3)? as usize,
                    canvas_label: row.get(4)?,
                    image_url: row.get(5)?,
                    local_path: row.get(6)?,
                    status: row.get(7)?,
                    ocr_status: row.get(8)?,
                    width: row.get::<_, Option<i64>>(9)?.map(|v| v as u32),
                    height: row.get::<_, Option<i64>>(10)?.map(|v| v as u32),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Get OCR results for a specific download.
    pub fn get_ocr_for_download(&self, download_id: i64) -> Result<Vec<OcrRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, backend, raw_text, structured_json, processed_at
             FROM ocr_results WHERE download_id = ?1",
        )?;
        let records = stmt
            .query_map(params![download_id], |row| {
                Ok(OcrRecord {
                    id: row.get(0)?,
                    backend: row.get(1)?,
                    raw_text: row.get(2)?,
                    structured_json: row.get(3)?,
                    processed_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Search manifests with pagination. Returns (results, total_count).
    pub fn search_manifests_paginated(
        &self,
        doc_type: Option<&str>,
        year: Option<&str>,
        archive_name: Option<&str>,
        locality: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<ManifestRecord>, usize)> {
        let mut where_clause = "WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(dt) = doc_type {
            where_clause.push_str(&format!(" AND m.doc_type = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(dt.to_string()));
        }
        if let Some(y) = year {
            where_clause.push_str(&format!(" AND m.year LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{y}%")));
        }
        if let Some(an) = archive_name {
            where_clause.push_str(&format!(" AND a.name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{an}%")));
        }
        if let Some(loc) = locality {
            where_clause.push_str(&format!(" AND m.archival_context LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{loc}%")));
        }

        // Count query
        let count_sql = format!(
            "SELECT COUNT(*) FROM manifests m LEFT JOIN archives a ON m.archive_db_id = a.id {where_clause}"
        );
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let total: i64 = self.conn.query_row(&count_sql, params_refs.as_slice(), |row| row.get(0))?;

        // Data query with pagination
        let data_sql = format!(
            "SELECT m.id, m.archive_id, m.title, m.total_canvases, m.doc_type,
                    m.archival_context, m.signature, m.date_from, m.date_to,
                    m.iiif_version, m.year, m.ark_url,
                    a.name as archive_name
             FROM manifests m
             LEFT JOIN archives a ON m.archive_db_id = a.id
             {where_clause}
             ORDER BY m.year, m.doc_type
             LIMIT ?{} OFFSET ?{}",
            params_vec.len() + 1,
            params_vec.len() + 2,
        );
        params_vec.push(Box::new(limit as i64));
        params_vec.push(Box::new(offset as i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&data_sql)?;
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(ManifestRecord {
                    id: row.get(0)?,
                    archive_id: row.get(1)?,
                    title: row.get(2)?,
                    total_canvases: row.get::<_, Option<i64>>(3)?.map(|v| v as usize),
                    doc_type: row.get(4)?,
                    archival_context: row.get(5)?,
                    signature: row.get(6)?,
                    date_from: row.get(7)?,
                    date_to: row.get(8)?,
                    iiif_version: row.get(9)?,
                    year: row.get(10)?,
                    ark_url: row.get(11)?,
                    archive_name: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((records, total as usize))
    }

    /// Get distinct document types.
    pub fn get_distinct_doc_types(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT doc_type FROM manifests WHERE doc_type IS NOT NULL ORDER BY doc_type",
        )?;
        let records = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Get distinct years.
    pub fn get_distinct_years(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT year FROM manifests WHERE year IS NOT NULL ORDER BY year",
        )?;
        let records = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Search persons with pagination. Returns (results, total_count).
    pub fn search_persons_paginated(
        &self,
        surname: Option<&str>,
        given_name: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<PersonRecord>, usize)> {
        let mut where_clause = "WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = surname {
            where_clause.push_str(&format!(" AND surname LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{s}%")));
        }
        if let Some(n) = given_name {
            where_clause.push_str(&format!(" AND given_name LIKE ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(format!("%{n}%")));
        }

        let count_sql = format!("SELECT COUNT(*) FROM persons {where_clause}");
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let total: i64 = self.conn.query_row(&count_sql, params_refs.as_slice(), |row| row.get(0))?;

        let data_sql = format!(
            "SELECT id, name, surname, given_name, detail_url, birth_info, death_info
             FROM persons {where_clause}
             ORDER BY surname, given_name
             LIMIT ?{} OFFSET ?{}",
            params_vec.len() + 1,
            params_vec.len() + 2,
        );
        params_vec.push(Box::new(limit as i64));
        params_vec.push(Box::new(offset as i64));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&data_sql)?;
        let records = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(PersonRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    surname: row.get(2)?,
                    given_name: row.get(3)?,
                    detail_url: row.get(4)?,
                    birth_info: row.get(5)?,
                    death_info: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((records, total as usize))
    }

    // ─── Cross-Record Linking ────────────────────────────────────────────

    /// Find potential cross-record links by matching surname+name tags across manifests.
    pub fn find_cross_record_candidates(
        &self,
        _min_score: f64,
        limit: usize,
    ) -> Result<Vec<CrossRecordCandidate>> {
        // Find (surname, given_name) pairs that appear in tags across multiple manifests
        let mut stmt = self.conn.prepare(
            "SELECT t1.value AS surname, t2.value AS given_name,
                    COUNT(DISTINCT d.manifest_id) AS manifest_count,
                    COUNT(DISTINCT d.id) AS record_count
             FROM tags t1
             JOIN tags t2 ON t1.download_id = t2.download_id
             JOIN downloads d ON t1.download_id = d.id
             WHERE t1.tag_type = 'surname' AND t2.tag_type = 'name'
             GROUP BY LOWER(t1.value), LOWER(t2.value)
             HAVING manifest_count > 1
             ORDER BY manifest_count DESC, record_count DESC
             LIMIT ?1",
        )?;

        let candidates = stmt
            .query_map(params![limit as i64], |row| {
                Ok(CrossRecordCandidateRow {
                    surname: row.get(0)?,
                    given_name: row.get(1)?,
                    manifest_count: row.get(2)?,
                    record_count: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // For each candidate, fetch the specific records
        let mut results = Vec::with_capacity(candidates.len());
        for c in candidates {
            let records = self.get_cross_record_details(&c.surname, &c.given_name)?;
            let score = c.manifest_count as f64 * 0.5 + c.record_count as f64 * 0.2;
            results.push(CrossRecordCandidate {
                surname: c.surname,
                given_name: c.given_name,
                manifest_count: c.manifest_count,
                record_count: c.record_count,
                score,
                records,
            });
        }

        Ok(results)
    }

    fn get_cross_record_details(&self, surname: &str, given_name: &str) -> Result<Vec<CrossRecordDetail>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT d.manifest_id, d.canvas_id, m.doc_type, m.year
             FROM tags t1
             JOIN tags t2 ON t1.download_id = t2.download_id
             JOIN downloads d ON t1.download_id = d.id
             JOIN manifests m ON d.manifest_id = m.id
             WHERE t1.tag_type = 'surname' AND LOWER(t1.value) = LOWER(?1)
               AND t2.tag_type = 'name' AND LOWER(t2.value) = LOWER(?2)
             ORDER BY m.year",
        )?;

        let records = stmt
            .query_map(params![surname, given_name], |row| {
                Ok(CrossRecordDetail {
                    manifest_id: row.get(0)?,
                    canvas_id: row.get(1)?,
                    doc_type: row.get(2)?,
                    year: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    // ─── Dashboard Methods ───────────────────────────────────────────────

    /// Get recent manifest download status for the TUI dashboard.
    pub fn get_recent_manifest_status(&self, limit: usize) -> Result<Vec<ManifestStatusRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.doc_type, m.year,
                    CASE
                        WHEN COUNT(d.id) = 0 THEN 'empty'
                        WHEN SUM(CASE WHEN d.status = 'complete' THEN 1 ELSE 0 END) = COUNT(d.id) THEN 'complete'
                        WHEN SUM(CASE WHEN d.status = 'failed' THEN 1 ELSE 0 END) > 0 THEN 'partial'
                        ELSE 'active'
                    END as status,
                    SUM(CASE WHEN d.status = 'complete' THEN 1 ELSE 0 END) as completed,
                    COUNT(d.id) as total
             FROM manifests m
             LEFT JOIN downloads d ON m.id = d.manifest_id
             GROUP BY m.id
             ORDER BY m.fetched_at DESC
             LIMIT ?1",
        )?;

        let records = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ManifestStatusRow {
                    id: row.get(0)?,
                    doc_type: row.get(1)?,
                    year: row.get(2)?,
                    status: row.get(3)?,
                    completed: row.get(4)?,
                    total: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
}

// ─── Data Structures ────────────────────────────────────────────────────

/// Input for upsert_manifest_full.
pub struct ManifestInsert<'a> {
    pub id: &'a str,
    pub archive_id: &'a str,
    pub title: Option<&'a str>,
    pub total_canvases: Option<usize>,
    pub json_cached: Option<&'a str>,
    pub ark_url: Option<&'a str>,
    pub doc_type: Option<&'a str>,
    pub archival_context: Option<&'a str>,
    pub archive_db_id: Option<i64>,
    pub locality_id: Option<i64>,
    pub signature: Option<&'a str>,
    pub date_from: Option<&'a str>,
    pub date_to: Option<&'a str>,
    pub license: Option<&'a str>,
    pub language: Option<&'a str>,
    pub iiif_version: Option<&'a str>,
    pub year: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub id: i64,
    pub canvas_id: String,
    pub canvas_index: usize,
    pub image_url: String,
    pub status: String,
}

/// Full download record with all fields for web API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FullDownloadRecord {
    pub id: i64,
    pub manifest_id: String,
    pub canvas_id: String,
    pub canvas_index: usize,
    pub canvas_label: Option<String>,
    pub image_url: String,
    pub local_path: Option<String>,
    pub status: String,
    pub ocr_status: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadStats {
    pub total: usize,
    pub complete: usize,
    pub failed: usize,
    pub pending: usize,
}

/// Record for a completed download (used by verify command).
pub struct CompletedDownload {
    pub manifest_id: String,
    pub canvas_id: String,
    pub local_path: String,
    pub sha256: String,
}

/// Batch entry for flushing download results in a single transaction.
pub struct DownloadResultBatch {
    pub manifest_id: String,
    pub canvas_id: String,
    pub local_path: String,
    pub sha256: String,
    pub error: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtendedStats {
    pub base: GlobalStats,
    pub archives: usize,
    pub localities: usize,
    pub persons: usize,
    pub search_queries: usize,
    pub registry_results: usize,
    pub registries: usize,
    pub ocr_results: usize,
}

/// Row from cross-record candidate query (internal).
struct CrossRecordCandidateRow {
    surname: String,
    given_name: String,
    manifest_count: i64,
    record_count: i64,
}

/// A cross-record candidate: a person appearing across multiple registries.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrossRecordCandidate {
    pub surname: String,
    pub given_name: String,
    pub manifest_count: i64,
    pub record_count: i64,
    pub score: f64,
    pub records: Vec<CrossRecordDetail>,
}

/// Detail of a single record for a cross-record candidate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrossRecordDetail {
    pub manifest_id: String,
    pub canvas_id: String,
    pub doc_type: Option<String>,
    pub year: Option<String>,
}

/// Row for dashboard manifest status.
pub struct ManifestStatusRow {
    pub id: String,
    pub doc_type: Option<String>,
    pub year: Option<String>,
    pub status: String,
    pub completed: i64,
    pub total: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ManifestRecord {
    pub id: String,
    pub archive_id: String,
    pub title: Option<String>,
    pub total_canvases: Option<usize>,
    pub doc_type: Option<String>,
    pub archival_context: Option<String>,
    pub signature: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub iiif_version: Option<String>,
    pub year: Option<String>,
    pub ark_url: Option<String>,
    pub archive_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveRecord {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalityRecord {
    pub id: i64,
    pub name: String,
    pub province: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryResultRecord {
    pub id: i64,
    pub ark_url: String,
    pub year: Option<String>,
    pub doc_type: Option<String>,
    pub signature: Option<String>,
    pub context: Option<String>,
    pub archive_name: Option<String>,
    pub archive_url: Option<String>,
    pub manifest_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryCatalogRecord {
    pub id: i64,
    pub ark_url: String,
    pub year: Option<String>,
    pub doc_type: Option<String>,
    pub signature: Option<String>,
    pub context: Option<String>,
    pub archive_name: Option<String>,
    pub locality_name: Option<String>,
    pub province: Option<String>,
    pub has_images: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegistryFacets {
    pub doc_types: Vec<String>,
    pub years: Vec<String>,
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonRecord {
    pub id: i64,
    pub name: String,
    pub surname: Option<String>,
    pub given_name: Option<String>,
    pub detail_url: Option<String>,
    pub birth_info: Option<String>,
    pub death_info: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonRecordEntry {
    pub id: i64,
    pub record_type: Option<String>,
    pub date: Option<String>,
    pub ark_url: Option<String>,
    pub manifest_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrRecord {
    pub id: i64,
    pub backend: String,
    pub raw_text: Option<String>,
    pub structured_json: Option<String>,
    pub processed_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrSearchResult {
    pub ocr_id: i64,
    pub download_id: i64,
    pub backend: String,
    pub snippet: String,
    pub manifest_id: String,
    pub canvas_id: String,
    pub canvas_index: usize,
    pub canvas_label: Option<String>,
    pub manifest_title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        let db = StateDb::in_memory().unwrap();
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

    #[test]
    fn test_archives() {
        let db = StateDb::in_memory().unwrap();

        let id1 = db.upsert_archive("Archivio di Stato di Lucca", "archivio-di-stato-di-lucca", None).unwrap();
        let id2 = db.upsert_archive("Archivio di Stato di Napoli", "archivio-di-stato-di-napoli", Some("https://example.com")).unwrap();
        assert_ne!(id1, id2);

        // Upsert same slug returns same id
        let id3 = db.upsert_archive("Archivio di Stato di Lucca", "archivio-di-stato-di-lucca", None).unwrap();
        assert_eq!(id1, id3);

        let archives = db.list_archives().unwrap();
        assert_eq!(archives.len(), 2);
    }

    #[test]
    fn test_localities() {
        let db = StateDb::in_memory().unwrap();

        let id1 = db.upsert_locality("Camposano", Some("NA")).unwrap();
        let id2 = db.upsert_locality("Lucca", Some("LU")).unwrap();
        assert_ne!(id1, id2);

        let results = db.search_localities("campo").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Camposano");
    }

    #[test]
    fn test_search_query_caching() {
        let db = StateDb::in_memory().unwrap();

        let query_id = db.insert_search_query("registry", "{\"locality\":\"Camposano\"}", Some(42), Some(1)).unwrap();
        assert!(query_id > 0);

        let result = RegistryResult {
            ark_url: "https://antenati.cultura.gov.it/ark:/12657/an_ua18771".to_string(),
            year: "1810".to_string(),
            doc_type: "Nati".to_string(),
            signature: "82.1422".to_string(),
            context: "Stato civile napoleonico > Camposano".to_string(),
            archive: "Archivio di Stato di Napoli".to_string(),
            archive_url: None,
        };

        db.insert_registry_result(query_id, &result).unwrap();

        let results = db.search_registry_results(Some("Nati"), None, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].year, Some("1810".to_string()));
    }

    #[test]
    fn test_persons() {
        let db = StateDb::in_memory().unwrap();

        let name_result = NameResult {
            name: "ROSSI Mario".to_string(),
            detail_url: "/detail-nominative/?s_id=123".to_string(),
            birth_info: Some("Camposano, 1810".to_string()),
            death_info: None,
            records: vec![
                crate::models::search::LinkedRecord {
                    record_type: "Atto di nascita".to_string(),
                    date: Some("1810".to_string()),
                    ark_url: Some("ark:/12657/an_ua18771".to_string()),
                },
            ],
        };

        let person_id = db.upsert_person(&name_result).unwrap();
        assert!(person_id > 0);

        let persons = db.search_persons(Some("ROSSI"), None).unwrap();
        assert_eq!(persons.len(), 1);
        assert_eq!(persons[0].name, "ROSSI Mario");

        let records = db.get_person_records(person_id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_type, Some("Atto di nascita".to_string()));
    }

    #[test]
    fn test_manifest_metadata() {
        let db = StateDb::in_memory().unwrap();
        db.upsert_manifest("m1", "a1", Some("Test"), 1, None).unwrap();

        let metadata = vec![
            MetadataEntry { label: "Tipologia".to_string(), value: "Nati".to_string() },
            MetadataEntry { label: "Contesto archivistico".to_string(), value: "Stato civile > Camposano".to_string() },
        ];

        db.store_manifest_metadata("m1", &metadata).unwrap();

        let result = db.get_manifest_metadata("m1").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_extended_stats() {
        let db = StateDb::in_memory().unwrap();
        let stats = db.get_extended_stats().unwrap();
        assert_eq!(stats.base.manifests, 0);
        assert_eq!(stats.archives, 0);
        assert_eq!(stats.persons, 0);
    }

    #[test]
    fn test_insert_download_full() {
        let db = StateDb::in_memory().unwrap();
        db.upsert_manifest("m1", "a1", Some("Test"), 1, None).unwrap();

        let dl_id = db.insert_download_full(
            "m1", "c1", 0, "http://example.com/1.jpg",
            Some("pag. 1"), Some(4000), Some(3000),
        ).unwrap();
        assert!(dl_id > 0);
    }

    #[test]
    fn test_search_manifests() {
        let db = StateDb::in_memory().unwrap();

        let archive_id = db.upsert_archive("Archivio di Stato di Napoli", "archivio-di-stato-di-napoli", None).unwrap();

        let insert = ManifestInsert {
            id: "m1",
            archive_id: "context",
            title: Some("Registro Nati 1810"),
            total_canvases: Some(50),
            json_cached: None,
            ark_url: Some("ark:/12657/an_ua18771"),
            doc_type: Some("Nati"),
            archival_context: Some("Stato civile napoleonico > Camposano"),
            archive_db_id: Some(archive_id),
            locality_id: None,
            signature: Some("82.1422"),
            date_from: Some("1810"),
            date_to: Some("1810"),
            license: None,
            language: None,
            iiif_version: Some("v2"),
            year: Some("1810"),
        };

        db.upsert_manifest_full(&insert).unwrap();

        let results = db.search_manifests(Some("Nati"), None, None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_type, Some("Nati".to_string()));

        let results = db.search_manifests(None, None, Some("Napoli"), None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_extract_year() {
        assert_eq!(extract_year("1810/01/01"), Some("1810".to_string()));
        assert_eq!(extract_year("1810"), Some("1810".to_string()));
        assert_eq!(extract_year("  1810/12/31 "), Some("1810".to_string()));
        assert_eq!(extract_year("Registro 37.1"), None);
        assert_eq!(extract_year(""), None);
    }

    #[test]
    fn test_parse_context_locality() {
        let (loc, prov) = parse_context_locality("Stato civile napoleonico > Camposano (provincia di Napoli)");
        assert_eq!(loc, Some("Camposano".to_string()));
        assert_eq!(prov, Some("Napoli".to_string()));

        let (loc, prov) = parse_context_locality("Stato civile > Roma");
        assert_eq!(loc, Some("Roma".to_string()));
        assert_eq!(prov, None);

        let (loc, prov) = parse_context_locality("");
        assert_eq!(loc, None);
        assert_eq!(prov, None);

        let (loc, prov) = parse_context_locality("SinglePart");
        assert_eq!(loc, Some("SinglePart".to_string()));
        assert_eq!(prov, None);
    }

    #[test]
    fn test_upsert_registry() {
        let db = StateDb::in_memory().unwrap();

        let result = RegistryResult {
            ark_url: "https://antenati.cultura.gov.it/ark:/12657/an_ua18771".to_string(),
            year: "1810".to_string(),
            doc_type: "Nati".to_string(),
            signature: "82.1422".to_string(),
            context: "Stato civile napoleonico > Camposano (provincia di Napoli)".to_string(),
            archive: "Archivio di Stato di Napoli".to_string(),
            archive_url: Some("https://antenati.cultura.gov.it/archivio/archivio-di-stato-di-napoli".to_string()),
        };

        let id1 = db.upsert_registry(&result).unwrap();
        assert!(id1 > 0);

        // Upsert same ark_url should return same id
        let id2 = db.upsert_registry(&result).unwrap();
        assert_eq!(id1, id2);

        // Update with different data
        let result2 = RegistryResult {
            year: "1811".to_string(),
            ..result.clone()
        };
        let id3 = db.upsert_registry(&result2).unwrap();
        assert_eq!(id1, id3);

        // Verify the data was updated
        let (records, total) = db.search_registries_catalog(None, None, None, None, None, 0, 50).unwrap();
        assert_eq!(total, 1);
        assert_eq!(records[0].year, Some("1811".to_string()));
        assert_eq!(records[0].locality_name, Some("Camposano".to_string()));
        assert_eq!(records[0].province, Some("Napoli".to_string()));

        // Verify archive was also created
        let archives = db.list_archives().unwrap();
        assert_eq!(archives.len(), 1);
        assert_eq!(archives[0].name, "Archivio di Stato di Napoli");
    }

    #[test]
    fn test_registries_batch() {
        let db = StateDb::in_memory().unwrap();

        let results = vec![
            RegistryResult {
                ark_url: "ark:/12657/an_ua001".to_string(),
                year: "1810".to_string(),
                doc_type: "Nati".to_string(),
                signature: "1.1".to_string(),
                context: "Civile > Napoli (provincia di Napoli)".to_string(),
                archive: "Archivio Napoli".to_string(),
                archive_url: None,
            },
            RegistryResult {
                ark_url: "ark:/12657/an_ua002".to_string(),
                year: "1820".to_string(),
                doc_type: "Morti".to_string(),
                signature: "2.1".to_string(),
                context: "Civile > Roma".to_string(),
                archive: "Archivio Roma".to_string(),
                archive_url: None,
            },
        ];

        let count = db.upsert_registries_batch(&results).unwrap();
        assert_eq!(count, 2);

        let (records, total) = db.search_registries_catalog(None, None, None, None, None, 0, 50).unwrap();
        assert_eq!(total, 2);
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn test_registries_has_images_filter() {
        let db = StateDb::in_memory().unwrap();

        let result = RegistryResult {
            ark_url: "ark:/12657/an_ua100".to_string(),
            year: "1810".to_string(),
            doc_type: "Nati".to_string(),
            signature: "1.1".to_string(),
            context: "Civile > Camposano (provincia di Napoli)".to_string(),
            archive: "Archivio".to_string(),
            archive_url: None,
        };
        db.upsert_registry(&result).unwrap();

        // No images yet
        let (records, _) = db.search_registries_catalog(None, None, None, None, Some(true), 0, 50).unwrap();
        assert_eq!(records.len(), 0);
        let (records, _) = db.search_registries_catalog(None, None, None, None, Some(false), 0, 50).unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].has_images);

        // Add a manifest with same ark_url and a complete download
        db.upsert_manifest("m1", "ctx", Some("Test"), 10, None).unwrap();
        // Set the ark_url on the manifest
        db.conn.execute(
            "UPDATE manifests SET ark_url = ?1 WHERE id = ?2",
            params!["ark:/12657/an_ua100", "m1"],
        ).unwrap();
        db.insert_download("m1", "c1", 0, "http://example.com/img.jpg").unwrap();
        db.mark_complete("m1", "c1", "/tmp/img.jpg", "abc123").unwrap();

        // Now should have images
        let (records, _) = db.search_registries_catalog(None, None, None, None, Some(true), 0, 50).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].has_images);

        let (records, _) = db.search_registries_catalog(None, None, None, None, Some(false), 0, 50).unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_registries_pagination() {
        let db = StateDb::in_memory().unwrap();

        for i in 0..10 {
            let result = RegistryResult {
                ark_url: format!("ark:/12657/an_ua{i:03}"),
                year: format!("{}", 1800 + i),
                doc_type: "Nati".to_string(),
                signature: format!("{i}.1"),
                context: "Civile > Napoli".to_string(),
                archive: "Archivio".to_string(),
                archive_url: None,
            };
            db.upsert_registry(&result).unwrap();
        }

        let (records, total) = db.search_registries_catalog(None, None, None, None, None, 0, 3).unwrap();
        assert_eq!(total, 10);
        assert_eq!(records.len(), 3);

        let (records, total) = db.search_registries_catalog(None, None, None, None, None, 9, 3).unwrap();
        assert_eq!(total, 10);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn test_registry_facets() {
        let db = StateDb::in_memory().unwrap();

        let results = vec![
            RegistryResult {
                ark_url: "ark:/12657/an_ua001".to_string(),
                year: "1810".to_string(),
                doc_type: "Nati".to_string(),
                signature: "1.1".to_string(),
                context: "Civile > Napoli".to_string(),
                archive: "Archivio Napoli".to_string(),
                archive_url: None,
            },
            RegistryResult {
                ark_url: "ark:/12657/an_ua002".to_string(),
                year: "1820".to_string(),
                doc_type: "Morti".to_string(),
                signature: "2.1".to_string(),
                context: "Civile > Roma".to_string(),
                archive: "Archivio Roma".to_string(),
                archive_url: None,
            },
        ];
        db.upsert_registries_batch(&results).unwrap();

        let facets = db.get_registry_facets().unwrap();
        assert_eq!(facets.doc_types, vec!["Morti", "Nati"]);
        assert_eq!(facets.years, vec!["1810", "1820"]);
        assert_eq!(facets.archives, vec!["Archivio Napoli", "Archivio Roma"]);
    }
}
