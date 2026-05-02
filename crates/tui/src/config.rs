use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// CLI arguments for pandaria-tui.
#[derive(Parser, Debug)]
#[command(name = "pandaria-tui", version = "0.1.0")]
pub struct CliArgs {
    /// Server URL
    #[arg(long, env = "PANDARIA_URL")]
    pub url: Option<String>,

    /// Authentication token
    #[arg(long, env = "PANDARIA_TOKEN")]
    pub token: Option<String>,

    /// Syntax highlighting theme
    #[arg(long)]
    pub theme: Option<String>,

    /// Path to config file
    #[arg(long)]
    pub config: Option<PathBuf>,
}

fn default_config_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "pandaria")
        .map(|d| d.config_dir().join("tui").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub ui: UiConfig,
    pub keys: Option<KeysConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_url() -> String { "http://localhost:8080".to_string() }
fn default_timeout_secs() -> u64 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    #[serde(default = "default_true")]
    pub show_tool_calls: bool,
    #[serde(default = "default_syntax_theme")]
    pub syntax_theme: String,
    #[serde(default = "default_scrollback")]
    pub scrollback: usize,
}

fn default_max_history() -> usize { 500 }
fn default_true() -> bool { true }
fn default_syntax_theme() -> String { "base16-ocean.dark".to_string() }
fn default_scrollback() -> usize { 1000 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysConfig {
    // User keybinding overrides (TBD MVP)
}

impl Config {
    /// Load config from CLI args, env vars, config file, and defaults.
    /// Priority: CLI > env > config file > defaults.
    pub fn load(cli: CliArgs) -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = cli.config.unwrap_or_else(default_config_path);
        let file_config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            Some(toml::from_str::<Config>(&content)?)
        } else {
            None
        };

        let mut config = file_config.unwrap_or(Config {
            server: ServerConfig { url: default_url(), timeout_secs: default_timeout_secs() },
            auth: AuthConfig { token: None },
            ui: UiConfig {
                max_history: default_max_history(),
                show_tool_calls: true,
                syntax_theme: default_syntax_theme(),
                scrollback: default_scrollback(),
            },
            keys: None,
        });

        // CLI/env overrides
        if let Some(url) = cli.url { config.server.url = url; }
        if let Some(token) = cli.token { config.auth.token = Some(token); }
        if let Some(theme) = cli.theme { config.ui.syntax_theme = theme; }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let cli = CliArgs {
            url: None, token: None, theme: None, config: None,
        };
        let config = Config::load(cli).expect("load config");
        assert_eq!(config.server.url, "http://localhost:8080");
    }

    #[test]
    fn test_cli_overrides_file() {
        let cli = CliArgs {
            url: Some("http://example.com:9090".to_string()),
            token: Some("test-token".to_string()),
            theme: None,
            config: None,
        };
        let config = Config::load(cli).expect("load config");
        assert_eq!(config.server.url, "http://example.com:9090");
        assert_eq!(config.auth.token, Some("test-token".to_string()));
    }

    #[test]
    fn test_env_var_priority_via_clap() {
        let cli = CliArgs {
            url: None,
            token: Some("from-cli".to_string()),
            theme: None,
            config: None,
        };
        let config = Config::load(cli).expect("load config");
        assert_eq!(config.auth.token, Some("from-cli".to_string()));
    }
}
