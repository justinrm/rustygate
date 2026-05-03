use std::{fs, path::PathBuf};

use rustygate::config::{AppConfig, ProviderKind};
use uuid::Uuid;

#[test]
fn config_parses_valid_toml_file() {
    let path = temp_config_path();
    fs::write(
        &path,
        r#"
[server]
host = "127.0.0.1"
port = 8080

[gateway]
default_timeout_ms = 30000
max_retries = 1
enable_request_logging = true
log_prompt_content = false

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
failure_rate = 0.0
base_latency_ms = 0
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.server.port, 8080);
    assert_eq!(config.gateway.max_retries, 1);
    assert!(!config.storage.enabled);
    assert_eq!(config.storage.database_url, "sqlite://rustygate.db");
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].name, "mock-fast");
    assert!(matches!(&config.providers[0].kind, ProviderKind::Mock));
}

#[test]
fn config_parses_optional_storage_settings() {
    let path = temp_config_path();
    fs::write(
        &path,
        r#"
[server]
host = "127.0.0.1"
port = 8080

[gateway]
default_timeout_ms = 30000
max_retries = 1
enable_request_logging = true
log_prompt_content = false

[storage]
enabled = true
database_url = "sqlite://tmp/rustygate-test.db"
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert!(config.storage.enabled);
    assert_eq!(
        config.storage.database_url,
        "sqlite://tmp/rustygate-test.db"
    );
}

#[test]
fn config_returns_error_for_invalid_toml_file() {
    let path = temp_config_path();
    fs::write(&path, "not valid toml =").unwrap();

    let result = AppConfig::from_file(&path);
    fs::remove_file(&path).unwrap();

    assert!(result.is_err());
}

#[test]
fn config_parses_real_provider_kinds_and_optional_fields() {
    let path = temp_config_path();
    fs::write(
        &path,
        r#"
[server]
host = "127.0.0.1"
port = 8080

[gateway]
default_timeout_ms = 30000
max_retries = 1
enable_request_logging = true
log_prompt_content = false

[[providers]]
name = "openai-live"
kind = "openai_compatible"
model = "gpt-4o-mini"
priority = 1
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
cost_per_1k_input_tokens = 0.001
cost_per_1k_output_tokens = 0.002

[[providers]]
name = "anthropic-live"
kind = "anthropic"
model = "claude-3-5-sonnet-latest"
priority = 2
api_key_env = "ANTHROPIC_API_KEY"
cost_per_1k_input_tokens = 0.003
cost_per_1k_output_tokens = 0.004
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.providers.len(), 2);
    assert!(matches!(
        config.providers[0].kind,
        ProviderKind::OpenaiCompatible
    ));
    assert_eq!(
        config.providers[0].base_url.as_deref(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(
        config.providers[0].api_key_env.as_deref(),
        Some("OPENAI_API_KEY")
    );
    assert!(matches!(config.providers[1].kind, ProviderKind::Anthropic));
    assert_eq!(config.providers[1].failure_rate, 0.0);
    assert_eq!(config.providers[1].base_latency_ms, 0);
}

fn temp_config_path() -> PathBuf {
    std::env::temp_dir().join(format!("rustygate-test-{}.toml", Uuid::new_v4()))
}
