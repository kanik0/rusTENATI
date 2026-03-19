use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RustenatiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("AWS WAF challenge encountered")]
    WafChallenge { challenge_url: String, body: String },

    #[error("Rate limited (HTTP 429), retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("IIIF manifest parse error: {0}")]
    ManifestParse(String),

    #[error("Image not found: {canvas_id}")]
    ImageNotFound { canvas_id: String },

    #[error("OCR backend error ({backend}): {message}")]
    Ocr { backend: String, message: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("State database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error: {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Invalid ARK identifier: {0}")]
    InvalidArk(String),

    #[error("Search returned no results")]
    NoResults,

    #[error("Unexpected HTTP status {status} for {url}")]
    UnexpectedStatus { status: u16, url: String },
}

impl RustenatiError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Returns true if this error is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Http(_)
                | Self::WafChallenge { .. }
                | Self::RateLimited { .. }
                | Self::UnexpectedStatus { status: 500..=599, .. }
        )
    }
}
