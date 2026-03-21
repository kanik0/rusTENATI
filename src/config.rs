use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub download: DownloadConfig,
    pub http: HttpConfig,
    pub ocr: OcrConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadConfig {
    pub concurrency: usize,
    pub delay_ms: u64,
    pub quality: String,
    pub format: String,
    pub resume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    pub user_agent: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
    /// Max idle connections per host in the pool
    pub pool_max_idle_per_host: usize,
    /// Connection pool idle timeout in seconds
    pub pool_idle_timeout_secs: u64,
    /// Enable TCP keepalive (seconds)
    pub tcp_keepalive_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    pub default_backend: String,
    pub language: String,
    pub concurrency: usize,
    pub claude: ClaudeOcrConfig,
    pub transkribus: TranskribusOcrConfig,
    pub azure: AzureOcrConfig,
    pub google: GoogleOcrConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ClaudeOcrConfig {
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TranskribusOcrConfig {
    #[serde(alias = "api_key")]
    pub access_token: String,
    /// Numeric HTR model ID for the metagrapho API
    pub htr_id: i64,
    /// Legacy model_id string (parsed as integer fallback for htr_id)
    #[serde(default)]
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AzureOcrConfig {
    pub endpoint: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GoogleOcrConfig {
    pub credentials_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download: DownloadConfig::default(),
            http: HttpConfig::default(),
            ocr: OcrConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            delay_ms: 500,
            quality: "full".into(),
            format: "jpg".into(),
            resume: true,
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            user_agent: format!("Mozilla/5.0 (compatible; Rustenati/{})", env!("CARGO_PKG_VERSION")),
            timeout_secs: 30,
            max_retries: 5,
            pool_max_idle_per_host: 10,
            pool_idle_timeout_secs: 90,
            tcp_keepalive_secs: Some(60),
        }
    }
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            default_backend: "claude".into(),
            language: "it".into(),
            concurrency: 2,
            claude: ClaudeOcrConfig {
                api_key: String::new(),
                model: "claude-sonnet-4-6".into(),
            },
            transkribus: TranskribusOcrConfig {
                access_token: String::new(),
                htr_id: 0,
                model_id: String::new(),
            },
            azure: AzureOcrConfig::default(),
            google: GoogleOcrConfig::default(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

impl Config {
    /// Load config from file, falling back to defaults for missing fields.
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read config file: {}", path.display()))?;
            let config: Config = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Return the default config file path (~/.config/rustenati/config.toml).
    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("", "", "rustenati")
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    /// Generate example config TOML string.
    pub fn example_toml() -> Result<String> {
        let config = Self::default();
        toml::to_string_pretty(&config).context("Failed to serialize default config")
    }
}
