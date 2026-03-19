use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config::Config;

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Create default config file
    Init,

    /// Show current config
    Show,

    /// Set a config value
    Set {
        /// Config key (dot-separated, e.g. download.concurrency)
        key: String,

        /// Config value
        value: String,
    },
}

pub fn run(action: &ConfigAction, config: &Config) -> Result<()> {
    match action {
        ConfigAction::Init => {
            let path = Config::default_path();
            if path.exists() {
                anyhow::bail!("Config file already exists at {}", path.display());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
            }
            let content = Config::example_toml()?;
            std::fs::write(&path, &content)
                .with_context(|| format!("Failed to write config file: {}", path.display()))?;
            println!("Config file created at {}", path.display());
            Ok(())
        }
        ConfigAction::Show => {
            let content = toml::to_string_pretty(config)
                .context("Failed to serialize config")?;
            println!("{content}");
            Ok(())
        }
        ConfigAction::Set { key, value } => {
            let path = Config::default_path();
            let mut config = if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read config: {}", path.display()))?;
                toml::from_str::<toml::Value>(&content)
                    .with_context(|| "Failed to parse config")?
            } else {
                // Start from default
                let default_str = Config::example_toml()?;
                toml::from_str::<toml::Value>(&default_str)?
            };

            // Navigate dot-separated key and set value
            let parts: Vec<&str> = key.split('.').collect();
            set_nested_value(&mut config, &parts, value)?;

            // Write back
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = toml::to_string_pretty(&config)
                .context("Failed to serialize config")?;
            std::fs::write(&path, &content)
                .with_context(|| format!("Failed to write config: {}", path.display()))?;

            println!("Set {key} = {value}");
            println!("Config saved to {}", path.display());
            Ok(())
        }
    }
}

fn set_nested_value(root: &mut toml::Value, keys: &[&str], value: &str) -> Result<()> {
    if keys.is_empty() {
        anyhow::bail!("Empty config key");
    }

    if keys.len() == 1 {
        let table = root
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("Config root is not a table"))?;
        table.insert(keys[0].to_string(), parse_toml_value(value));
        return Ok(());
    }

    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Config root is not a table"))?;

    if !table.contains_key(keys[0]) {
        table.insert(keys[0].to_string(), toml::Value::Table(toml::map::Map::new()));
    }

    let child = table
        .get_mut(keys[0])
        .ok_or_else(|| anyhow::anyhow!("Key '{}' not found", keys[0]))?;

    set_nested_value(child, &keys[1..], value)
}

fn parse_toml_value(s: &str) -> toml::Value {
    // Try integer
    if let Ok(n) = s.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    // Try float
    if let Ok(f) = s.parse::<f64>() {
        return toml::Value::Float(f);
    }
    // Try boolean
    match s {
        "true" => return toml::Value::Boolean(true),
        "false" => return toml::Value::Boolean(false),
        _ => {}
    }
    // Default: string
    toml::Value::String(s.to_string())
}
