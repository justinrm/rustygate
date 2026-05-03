use std::{env, fs, net::IpAddr, path::Path};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub default_timeout_ms: u64,
    pub max_retries: u32,
    pub enable_request_logging: bool,
    pub log_prompt_content: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_database_url")]
    pub database_url: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            database_url: default_database_url(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    pub model: String,
    pub priority: u32,
    #[serde(default = "default_failure_rate")]
    pub failure_rate: f64,
    #[serde(default)]
    pub base_latency_ms: u64,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    pub cost_per_1k_input_tokens: f64,
    pub cost_per_1k_output_tokens: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Mock,
    OpenaiCompatible,
    Anthropic,
}

fn default_database_url() -> String {
    "sqlite://rustygate.db".into()
}

fn default_failure_rate() -> f64 {
    0.0
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Read(#[from] std::io::Error),
    #[error("failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let path =
            env::var("RUSTYGATE_CONFIG").unwrap_or_else(|_| "config/gateway.example.toml".into());
        Self::from_file(path)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }
}
