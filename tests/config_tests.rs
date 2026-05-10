use std::{fs, path::PathBuf};

use rustygate::config::{AppConfig, ProviderKind, RoutingPolicy};
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
shutdown_grace_period_ms = 12000

[gateway]
default_timeout_ms = 30000
max_retries = 1
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[gateway.retry]
initial_backoff_ms = 75
max_backoff_ms = 900
jitter_ms = 30

[gateway.circuit_breaker]
failure_threshold = 4
open_duration_ms = 8000
half_open_max_probes = 2

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
failure_rate = 0.0
base_latency_ms = 0
timeout_ms = 1234
max_retries = 2
retry_initial_backoff_ms = 10
retry_max_backoff_ms = 100
retry_jitter_ms = 5
circuit_breaker_failure_threshold = 6
circuit_breaker_open_duration_ms = 12000
circuit_breaker_half_open_max_probes = 3
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.server.port, 8080);
    assert_eq!(config.server.shutdown_grace_period_ms, 12_000);
    assert_eq!(config.gateway.max_retries, 1);
    assert_eq!(config.gateway.stream_idle_timeout_ms, 30_000);
    assert_eq!(config.gateway.routing_policy, RoutingPolicy::Priority);
    assert!(config.gateway.model_aliases.is_empty());
    assert_eq!(config.gateway.retry.initial_backoff_ms, 75);
    assert_eq!(config.gateway.retry.max_backoff_ms, 900);
    assert_eq!(config.gateway.retry.jitter_ms, 30);
    assert_eq!(config.gateway.circuit_breaker.failure_threshold, 4);
    assert_eq!(config.gateway.circuit_breaker.open_duration_ms, 8_000);
    assert_eq!(config.gateway.circuit_breaker.half_open_max_probes, 2);
    assert_eq!(config.gateway.api_key_env, "RUSTYGATE_GATEWAY_API_KEY");
    assert_eq!(config.gateway.rate_limit.global_requests_per_minute, 120);
    assert_eq!(config.gateway.rate_limit.global_burst_size, 30);
    assert_eq!(config.gateway.rate_limit.per_key_requests_per_minute, 60);
    assert_eq!(config.gateway.rate_limit.per_key_burst_size, 20);
    assert_eq!(config.gateway.request_limits.max_chat_body_bytes, 65_536);
    assert_eq!(config.gateway.request_limits.max_messages_per_request, 64);
    assert_eq!(
        config.gateway.request_limits.max_message_content_chars,
        8_000
    );
    assert_eq!(config.gateway.admission.max_global_in_flight, None);
    assert_eq!(config.gateway.admission.max_estimated_prompt_tokens, None);
    assert_eq!(config.gateway.admission.max_estimated_total_tokens, None);
    assert_eq!(config.gateway.admission.retry_after_seconds, 1);
    assert!(config.gateway.route_exposure.placeholder_compat_routes);
    assert!(!config.storage.enabled);
    assert_eq!(config.storage.database_url, "sqlite://rustygate.db");
    assert_eq!(config.providers.len(), 1);
    assert!(config.model_pools.is_empty());
    assert_eq!(config.providers[0].name, "mock-fast");
    assert!(matches!(&config.providers[0].kind, ProviderKind::Mock));
    assert_eq!(config.providers[0].timeout_ms, Some(1_234));
    assert_eq!(config.providers[0].max_retries, Some(2));
    assert_eq!(config.providers[0].retry_initial_backoff_ms, Some(10));
    assert_eq!(config.providers[0].retry_max_backoff_ms, Some(100));
    assert_eq!(config.providers[0].retry_jitter_ms, Some(5));
    assert_eq!(
        config.providers[0].circuit_breaker_failure_threshold,
        Some(6)
    );
    assert_eq!(
        config.providers[0].circuit_breaker_open_duration_ms,
        Some(12_000)
    );
    assert_eq!(
        config.providers[0].circuit_breaker_half_open_max_probes,
        Some(3)
    );
    assert_eq!(config.providers[0].max_in_flight, None);
}

#[test]
fn config_parses_route_exposure_settings() {
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
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[gateway.route_exposure]
placeholder_compat_routes = false

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert!(!config.gateway.route_exposure.placeholder_compat_routes);
}

#[test]
fn config_parses_admission_limits() {
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
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[gateway.admission]
max_global_in_flight = 10
max_estimated_prompt_tokens = 4096
max_estimated_total_tokens = 8192
retry_after_seconds = 3

[[providers]]
name = "replica-a"
kind = "mock"
model = "internal-replica-a"
priority = 1
max_in_flight = 2
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast"
aliases = ["mock-fast-v1"]
members = ["replica-a"]
max_in_flight = 4
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.gateway.admission.max_global_in_flight, Some(10));
    assert_eq!(
        config.gateway.admission.max_estimated_prompt_tokens,
        Some(4096)
    );
    assert_eq!(
        config.gateway.admission.max_estimated_total_tokens,
        Some(8192)
    );
    assert_eq!(config.gateway.admission.retry_after_seconds, 3);
    assert_eq!(config.providers[0].max_in_flight, Some(2));
    assert_eq!(config.model_pools[0].max_in_flight, Some(4));
}

#[test]
fn config_parses_routing_policy_and_model_aliases() {
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
routing_policy = "cost"
model_aliases = { "gpt-4o" = "gpt-4o-mini" }
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[[providers]]
name = "mock-fast"
kind = "mock"
model = "gpt-4o-mini"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.gateway.routing_policy, RoutingPolicy::Cost);
    assert_eq!(
        config
            .gateway
            .model_aliases
            .get("gpt-4o")
            .map(String::as_str),
        Some("gpt-4o-mini")
    );
}

#[test]
fn config_parses_prefix_affinity_settings() {
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
routing_policy = "prefix_affinity"
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[gateway.prefix_affinity]
ttl_seconds = 120
max_entries = 256
load_imbalance_threshold = 3
fallback_policy = "priority"

[[providers]]
name = "replica-a"
kind = "mock"
model = "internal-a"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[providers]]
name = "replica-b"
kind = "mock"
model = "internal-b"
priority = 2
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast"
routing_policy = "prefix_affinity"
members = ["replica-a", "replica-b"]
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.gateway.routing_policy, RoutingPolicy::PrefixAffinity);
    assert_eq!(config.gateway.prefix_affinity.ttl_seconds, 120);
    assert_eq!(config.gateway.prefix_affinity.max_entries, 256);
    assert_eq!(config.gateway.prefix_affinity.load_imbalance_threshold, 3);
    assert_eq!(
        config.gateway.prefix_affinity.fallback_policy,
        RoutingPolicy::Priority
    );
    assert_eq!(
        config.model_pools[0].routing_policy,
        Some(RoutingPolicy::PrefixAffinity)
    );
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
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[storage]
enabled = true
database_url = "sqlite://tmp/rustygate-test.db"

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
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
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

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

#[test]
fn config_rejects_duplicate_provider_names() {
    let path = temp_config_path();
    fs::write(
        &path,
        format!(
            r#"
{}

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 2
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
            base_config_without_providers()
        ),
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains("duplicate provider name `mock-fast`"));
}

#[test]
fn config_rejects_invalid_limits_and_missing_auth_config() {
    let path = temp_config_path();
    fs::write(
        &path,
        r#"
[server]
host = "127.0.0.1"
port = 8080
shutdown_grace_period_ms = 0

[gateway]
default_timeout_ms = 0
stream_idle_timeout_ms = 0
max_retries = 1
enable_request_logging = true
log_prompt_content = false
api_key_env = ""

[gateway.rate_limit]
global_requests_per_minute = 0
global_burst_size = 30
per_key_requests_per_minute = 60
per_key_burst_size = 20

[gateway.request_limits]
max_chat_body_bytes = 0
max_messages_per_request = 64
max_message_content_chars = 8000

[gateway.prefix_affinity]
ttl_seconds = 0
max_entries = 0
fallback_policy = "prefix_affinity"

[gateway.admission]
max_global_in_flight = 0
max_estimated_prompt_tokens = 0
max_estimated_total_tokens = 0
retry_after_seconds = 0

[[providers]]
name = "openai-live"
kind = "openai_compatible"
model = "gpt-4o-mini"
priority = 1
max_in_flight = 0
cost_per_1k_input_tokens = 0.001
cost_per_1k_output_tokens = 0.002

[[model_pools]]
name = "gpt-4o"
members = ["openai-live"]
max_in_flight = 0
"#,
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains("server.shutdown_grace_period_ms must be greater than 0"));
    assert!(error.contains("gateway.default_timeout_ms must be greater than 0"));
    assert!(error.contains("gateway.stream_idle_timeout_ms must be greater than 0"));
    assert!(error.contains("gateway.api_key_env must not be empty"));
    assert!(error.contains("gateway.rate_limit.global_requests_per_minute"));
    assert!(error.contains("gateway.request_limits.max_chat_body_bytes"));
    assert!(error.contains("gateway.prefix_affinity.ttl_seconds"));
    assert!(error.contains("gateway.prefix_affinity.max_entries"));
    assert!(error.contains("gateway.prefix_affinity.fallback_policy"));
    assert!(error.contains("gateway.admission.max_global_in_flight"));
    assert!(error.contains("gateway.admission.max_estimated_prompt_tokens"));
    assert!(error.contains("gateway.admission.max_estimated_total_tokens"));
    assert!(error.contains("gateway.admission.retry_after_seconds"));
    assert!(error.contains("api_key_env is required for real providers"));
    assert!(error.contains("provider `openai-live` max_in_flight"));
    assert!(error.contains("model pool `gpt-4o` max_in_flight"));
}

#[test]
fn config_rejects_alias_target_without_provider_model() {
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
model_aliases = { "gpt-4o" = "gpt-4o-mini" }
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[[providers]]
name = "mock-fast"
kind = "mock"
model = "mock-fast-v1"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002
"#,
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains(
        "gateway.model_aliases target `gpt-4o-mini` must match a configured provider model or model pool"
    ));
}

#[test]
fn config_parses_model_pools_and_allows_alias_targeting_pool() {
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
model_aliases = { "public-model" = "mock-fast-pool" }
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[[providers]]
name = "mock-a"
kind = "mock"
model = "internal-replica-a"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[providers]]
name = "mock-b"
kind = "mock"
model = "internal-replica-b"
priority = 2
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast-pool"
aliases = ["mock-fast"]
members = ["mock-a", "mock-b"]
"#,
    )
    .unwrap();

    let config = AppConfig::from_file(&path).unwrap();
    fs::remove_file(&path).unwrap();

    assert_eq!(config.model_pools.len(), 1);
    assert_eq!(config.model_pools[0].name, "mock-fast-pool");
    assert_eq!(config.model_pools[0].aliases, vec!["mock-fast"]);
    assert_eq!(
        config
            .gateway
            .model_aliases
            .get("public-model")
            .map(String::as_str),
        Some("mock-fast-pool")
    );
}

#[test]
fn config_rejects_model_pool_without_members() {
    let path = temp_config_path();
    fs::write(
        &path,
        format!(
            r#"
{}

[[providers]]
name = "mock-a"
kind = "mock"
model = "internal-replica-a"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast-pool"
members = []
"#,
            base_config_without_providers()
        ),
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains("model pool `mock-fast-pool` must include at least one member provider"));
}

#[test]
fn config_rejects_model_pool_member_that_is_not_configured_provider() {
    let path = temp_config_path();
    fs::write(
        &path,
        format!(
            r#"
{}

[[providers]]
name = "mock-a"
kind = "mock"
model = "internal-replica-a"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast-pool"
members = ["missing-provider"]
"#,
            base_config_without_providers()
        ),
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains(
        "model pool `mock-fast-pool` member `missing-provider` must reference a configured provider"
    ));
}

#[test]
fn config_rejects_model_pool_alias_conflict_with_gateway_alias_key() {
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
model_aliases = { "mock-fast" = "internal-replica-a" }
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"

[[providers]]
name = "mock-a"
kind = "mock"
model = "internal-replica-a"
priority = 1
cost_per_1k_input_tokens = 0.0001
cost_per_1k_output_tokens = 0.0002

[[model_pools]]
name = "mock-fast-pool"
aliases = ["mock-fast"]
members = ["mock-a"]
"#,
    )
    .unwrap();

    let error = AppConfig::from_file(&path).unwrap_err().to_string();
    fs::remove_file(&path).unwrap();

    assert!(error.contains(
        "model pool `mock-fast-pool` alias `mock-fast` conflicts with gateway.model_aliases key `mock-fast`"
    ));
}

#[test]
fn deployment_profile_configs_parse_and_validate() {
    for path in [
        "config/gateway.local.toml",
        "config/gateway.staging.toml",
        "config/gateway.prod.toml",
    ] {
        AppConfig::from_file(path).unwrap_or_else(|error| {
            panic!("{path} should parse and validate: {error}");
        });
    }
}

fn temp_config_path() -> PathBuf {
    std::env::temp_dir().join(format!("rustygate-test-{}.toml", Uuid::new_v4()))
}

fn base_config_without_providers() -> &'static str {
    r#"
[server]
host = "127.0.0.1"
port = 8080

[gateway]
default_timeout_ms = 30000
max_retries = 1
enable_request_logging = true
log_prompt_content = false
api_key_env = "RUSTYGATE_GATEWAY_API_KEY"
"#
}
