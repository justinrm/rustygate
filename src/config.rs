use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    net::IpAddr,
    path::Path,
};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: IpAddr,
    pub port: u16,
    #[serde(default = "default_shutdown_grace_period_ms")]
    pub shutdown_grace_period_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub default_timeout_ms: u64,
    pub max_retries: u32,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default)]
    pub routing_policy: RoutingPolicy,
    #[serde(default)]
    pub model_aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub retry: GatewayRetryConfig,
    #[serde(default)]
    pub circuit_breaker: GatewayCircuitBreakerConfig,
    pub enable_request_logging: bool,
    pub log_prompt_content: bool,
    pub api_key_env: String,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    #[serde(default)]
    pub request_limits: RequestLimitsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_service_name")]
    pub service_name: String,
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    #[serde(default = "default_sample_ratio")]
    pub sample_ratio: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: CacheBackendConfig,
    #[serde(default = "default_cache_default_ttl_seconds")]
    pub default_ttl_seconds: u64,
    #[serde(default = "default_cache_max_entries")]
    pub max_entries: usize,
    #[serde(default)]
    pub semantic: SemanticCacheConfig,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: CacheBackendConfig::Memory,
            default_ttl_seconds: default_cache_default_ttl_seconds(),
            max_entries: default_cache_max_entries(),
            semantic: SemanticCacheConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackendConfig {
    #[default]
    Memory,
    Sqlite,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticCacheConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub embedding_provider: Option<String>,
    #[serde(default = "default_semantic_similarity_threshold")]
    pub similarity_threshold: f32,
    #[serde(default = "default_semantic_index_capacity")]
    pub index_capacity: usize,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_provider: None,
            similarity_threshold: default_semantic_similarity_threshold(),
            index_capacity: default_semantic_index_capacity(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: default_service_name(),
            otlp_endpoint: None,
            sample_ratio: default_sample_ratio(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingPolicy {
    #[default]
    Priority,
    Cost,
    Latency,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayRetryConfig {
    #[serde(default = "default_retry_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_retry_max_backoff_ms")]
    pub max_backoff_ms: u64,
    #[serde(default = "default_retry_jitter_ms")]
    pub jitter_ms: u64,
}

impl Default for GatewayRetryConfig {
    fn default() -> Self {
        Self {
            initial_backoff_ms: default_retry_initial_backoff_ms(),
            max_backoff_ms: default_retry_max_backoff_ms(),
            jitter_ms: default_retry_jitter_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayCircuitBreakerConfig {
    #[serde(default = "default_breaker_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_breaker_open_duration_ms")]
    pub open_duration_ms: u64,
    #[serde(default = "default_breaker_half_open_max_probes")]
    pub half_open_max_probes: u32,
}

impl Default for GatewayCircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_breaker_failure_threshold(),
            open_duration_ms: default_breaker_open_duration_ms(),
            half_open_max_probes: default_breaker_half_open_max_probes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default)]
    pub backend: RateLimitBackendConfig,
    #[serde(default = "default_global_requests_per_minute")]
    pub global_requests_per_minute: u32,
    #[serde(default = "default_global_burst_size")]
    pub global_burst_size: u32,
    #[serde(default = "default_per_key_requests_per_minute")]
    pub per_key_requests_per_minute: u32,
    #[serde(default = "default_per_key_burst_size")]
    pub per_key_burst_size: u32,
    #[serde(default = "default_max_tracked_keys")]
    pub max_tracked_keys: usize,
    #[serde(default)]
    pub redis_url_env: Option<String>,
    #[serde(default = "default_redis_fallback_to_local")]
    pub redis_fallback_to_local: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            backend: RateLimitBackendConfig::Local,
            global_requests_per_minute: default_global_requests_per_minute(),
            global_burst_size: default_global_burst_size(),
            per_key_requests_per_minute: default_per_key_requests_per_minute(),
            per_key_burst_size: default_per_key_burst_size(),
            max_tracked_keys: default_max_tracked_keys(),
            redis_url_env: None,
            redis_fallback_to_local: default_redis_fallback_to_local(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitBackendConfig {
    #[default]
    Local,
    Redis,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequestLimitsConfig {
    #[serde(default = "default_max_chat_body_bytes")]
    pub max_chat_body_bytes: usize,
    #[serde(default = "default_max_messages_per_request")]
    pub max_messages_per_request: usize,
    #[serde(default = "default_max_message_content_chars")]
    pub max_message_content_chars: usize,
}

impl Default for RequestLimitsConfig {
    fn default() -> Self {
        Self {
            max_chat_body_bytes: default_max_chat_body_bytes(),
            max_messages_per_request: default_max_messages_per_request(),
            max_message_content_chars: default_max_message_content_chars(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_retention_days")]
    pub retention_days: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            database_url: default_database_url(),
            retention_days: default_retention_days(),
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
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub retry_initial_backoff_ms: Option<u64>,
    #[serde(default)]
    pub retry_max_backoff_ms: Option<u64>,
    #[serde(default)]
    pub retry_jitter_ms: Option<u64>,
    #[serde(default)]
    pub circuit_breaker_failure_threshold: Option<u32>,
    #[serde(default)]
    pub circuit_breaker_open_duration_ms: Option<u64>,
    #[serde(default)]
    pub circuit_breaker_half_open_max_probes: Option<u32>,
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

fn default_retention_days() -> u64 {
    30
}

fn default_shutdown_grace_period_ms() -> u64 {
    10_000
}

fn default_failure_rate() -> f64 {
    0.0
}

fn default_service_name() -> String {
    "rustygate".into()
}

fn default_sample_ratio() -> f64 {
    1.0
}

fn default_cache_default_ttl_seconds() -> u64 {
    600
}

fn default_cache_max_entries() -> usize {
    10_000
}

fn default_semantic_similarity_threshold() -> f32 {
    0.95
}

fn default_semantic_index_capacity() -> usize {
    10_000
}

fn default_retry_initial_backoff_ms() -> u64 {
    100
}

fn default_health_check_interval_ms() -> u64 {
    30_000
}

fn default_retry_max_backoff_ms() -> u64 {
    2_000
}

fn default_retry_jitter_ms() -> u64 {
    50
}

fn default_breaker_failure_threshold() -> u32 {
    3
}

fn default_breaker_open_duration_ms() -> u64 {
    5_000
}

fn default_breaker_half_open_max_probes() -> u32 {
    1
}

fn default_global_requests_per_minute() -> u32 {
    120
}

fn default_global_burst_size() -> u32 {
    30
}

fn default_per_key_requests_per_minute() -> u32 {
    60
}

fn default_per_key_burst_size() -> u32 {
    20
}

fn default_max_tracked_keys() -> usize {
    10_000
}

fn default_redis_fallback_to_local() -> bool {
    true
}

fn default_max_chat_body_bytes() -> usize {
    65_536
}

fn default_max_messages_per_request() -> usize {
    64
}

fn default_max_message_content_chars() -> usize {
    8_000
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Read(#[from] std::io::Error),
    #[error("failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Validation(String),
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let path =
            env::var("RUSTYGATE_CONFIG").unwrap_or_else(|_| "config/gateway.example.toml".into());
        Self::from_file(path)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();

        if self.server.port == 0 {
            errors.push("server.port must be greater than 0".to_string());
        }
        if self.server.shutdown_grace_period_ms == 0 {
            errors.push("server.shutdown_grace_period_ms must be greater than 0".to_string());
        }

        validate_gateway_config(&self.gateway, &mut errors);
        validate_telemetry_config(&self.telemetry, &mut errors);
        validate_cache_config(&self.cache, &mut errors);
        validate_storage_config(&self.storage, &mut errors);
        validate_providers(&self.providers, &self.gateway, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Validation(errors.join("; ")))
        }
    }
}

fn validate_cache_config(cache: &CacheConfig, errors: &mut Vec<String>) {
    if cache.enabled && cache.default_ttl_seconds == 0 {
        errors
            .push("cache.default_ttl_seconds must be greater than 0 when cache is enabled".into());
    }
    if cache.enabled && cache.max_entries == 0 {
        errors.push("cache.max_entries must be greater than 0 when cache is enabled".into());
    }
    if cache.semantic.enabled && cache.semantic.embedding_provider.is_none() {
        errors.push(
            "cache.semantic.embedding_provider must be set when semantic cache is enabled".into(),
        );
    }
    if !(0.0..=1.0).contains(&cache.semantic.similarity_threshold) {
        errors.push("cache.semantic.similarity_threshold must be between 0.0 and 1.0".into());
    }
    if cache.semantic.enabled && cache.semantic.index_capacity == 0 {
        errors.push(
            "cache.semantic.index_capacity must be greater than 0 when semantic cache is enabled"
                .into(),
        );
    }
}

fn validate_telemetry_config(telemetry: &TelemetryConfig, errors: &mut Vec<String>) {
    if telemetry.service_name.trim().is_empty() {
        errors.push("telemetry.service_name must not be empty".into());
    }
    if !(0.0..=1.0).contains(&telemetry.sample_ratio) {
        errors.push("telemetry.sample_ratio must be between 0.0 and 1.0".into());
    }
    if telemetry
        .otlp_endpoint
        .as_deref()
        .is_some_and(|endpoint| endpoint.trim().is_empty())
    {
        errors.push("telemetry.otlp_endpoint must not be empty when set".into());
    }
}

fn validate_gateway_config(gateway: &GatewayConfig, errors: &mut Vec<String>) {
    if gateway.default_timeout_ms == 0 {
        errors.push("gateway.default_timeout_ms must be greater than 0".to_string());
    }
    if gateway.health_check_interval_ms == 0 {
        errors.push("gateway.health_check_interval_ms must be greater than 0".to_string());
    }
    if gateway.api_key_env.trim().is_empty() {
        errors.push("gateway.api_key_env must not be empty".to_string());
    }
    if gateway.retry.initial_backoff_ms == 0 {
        errors.push("gateway.retry.initial_backoff_ms must be greater than 0".to_string());
    }
    if gateway.retry.max_backoff_ms == 0 {
        errors.push("gateway.retry.max_backoff_ms must be greater than 0".to_string());
    }
    if gateway.retry.max_backoff_ms < gateway.retry.initial_backoff_ms {
        errors.push(
            "gateway.retry.max_backoff_ms must be greater than or equal to initial_backoff_ms"
                .to_string(),
        );
    }
    if gateway.circuit_breaker.failure_threshold == 0 {
        errors.push("gateway.circuit_breaker.failure_threshold must be greater than 0".to_string());
    }
    if gateway.circuit_breaker.open_duration_ms == 0 {
        errors.push("gateway.circuit_breaker.open_duration_ms must be greater than 0".to_string());
    }
    if gateway.circuit_breaker.half_open_max_probes == 0 {
        errors.push(
            "gateway.circuit_breaker.half_open_max_probes must be greater than 0".to_string(),
        );
    }

    if gateway.rate_limit.global_requests_per_minute == 0 {
        errors.push("gateway.rate_limit.global_requests_per_minute must be greater than 0".into());
    }
    if gateway.rate_limit.global_burst_size == 0 {
        errors.push("gateway.rate_limit.global_burst_size must be greater than 0".into());
    }
    if gateway.rate_limit.per_key_requests_per_minute == 0 {
        errors.push("gateway.rate_limit.per_key_requests_per_minute must be greater than 0".into());
    }
    if gateway.rate_limit.per_key_burst_size == 0 {
        errors.push("gateway.rate_limit.per_key_burst_size must be greater than 0".into());
    }
    if gateway.rate_limit.max_tracked_keys == 0 {
        errors.push("gateway.rate_limit.max_tracked_keys must be greater than 0".into());
    }
    if gateway.rate_limit.backend == RateLimitBackendConfig::Redis
        && gateway
            .rate_limit
            .redis_url_env
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        errors.push("gateway.rate_limit.redis_url_env must be set when backend is redis".into());
    }

    if gateway.request_limits.max_chat_body_bytes == 0 {
        errors.push("gateway.request_limits.max_chat_body_bytes must be greater than 0".into());
    }
    if gateway.request_limits.max_messages_per_request == 0 {
        errors
            .push("gateway.request_limits.max_messages_per_request must be greater than 0".into());
    }
    if gateway.request_limits.max_message_content_chars == 0 {
        errors
            .push("gateway.request_limits.max_message_content_chars must be greater than 0".into());
    }

    for (alias, target) in &gateway.model_aliases {
        if alias.trim().is_empty() {
            errors.push("gateway.model_aliases must not contain empty alias names".into());
        }
        if target.trim().is_empty() {
            errors.push(format!(
                "gateway.model_aliases entry `{alias}` must not have an empty target"
            ));
        }
    }
}

fn validate_storage_config(storage: &StorageConfig, errors: &mut Vec<String>) {
    if storage.enabled && storage.database_url.trim().is_empty() {
        errors.push("storage.database_url must not be empty when storage is enabled".to_string());
    }
    if storage.enabled && storage.retention_days == 0 {
        errors.push("storage.retention_days must be greater than 0 when storage is enabled".into());
    }
}

fn validate_providers(
    providers: &[ProviderConfig],
    gateway: &GatewayConfig,
    errors: &mut Vec<String>,
) {
    if providers.is_empty() {
        errors.push("at least one provider must be configured".to_string());
    }

    let mut names = HashSet::new();
    let models: HashSet<&str> = providers
        .iter()
        .map(|provider| provider.model.as_str())
        .collect();

    for target in gateway.model_aliases.values() {
        if !target.trim().is_empty() && !models.contains(target.as_str()) {
            errors.push(format!(
                "gateway.model_aliases target `{target}` must match a configured provider model"
            ));
        }
    }

    for provider in providers {
        if provider.name.trim().is_empty() {
            errors.push("provider name must not be empty".to_string());
        } else if !names.insert(provider.name.as_str()) {
            errors.push(format!("duplicate provider name `{}`", provider.name));
        }

        if provider.model.trim().is_empty() {
            errors.push(format!(
                "provider `{}` model must not be empty",
                provider.name
            ));
        }
        if provider.priority == 0 {
            errors.push(format!(
                "provider `{}` priority must be greater than 0",
                provider.name
            ));
        }
        if !(0.0..=1.0).contains(&provider.failure_rate) {
            errors.push(format!(
                "provider `{}` failure_rate must be between 0.0 and 1.0",
                provider.name
            ));
        }
        if provider.cost_per_1k_input_tokens < 0.0 {
            errors.push(format!(
                "provider `{}` cost_per_1k_input_tokens must not be negative",
                provider.name
            ));
        }
        if provider.cost_per_1k_output_tokens < 0.0 {
            errors.push(format!(
                "provider `{}` cost_per_1k_output_tokens must not be negative",
                provider.name
            ));
        }

        if provider
            .base_url
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            errors.push(format!(
                "provider `{}` base_url must not be empty",
                provider.name
            ));
        }
        if provider.timeout_ms == Some(0) {
            errors.push(format!(
                "provider `{}` timeout_ms must be greater than 0",
                provider.name
            ));
        }

        validate_provider_auth(provider, errors);
        validate_provider_retry(provider, gateway, errors);
        validate_provider_breaker(provider, gateway, errors);
    }
}

fn validate_provider_auth(provider: &ProviderConfig, errors: &mut Vec<String>) {
    match provider.kind {
        ProviderKind::Mock => {}
        ProviderKind::OpenaiCompatible | ProviderKind::Anthropic => {
            if provider
                .api_key_env
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                errors.push(format!(
                    "provider `{}` api_key_env is required for real providers",
                    provider.name
                ));
            }
        }
    }
}

fn validate_provider_retry(
    provider: &ProviderConfig,
    gateway: &GatewayConfig,
    errors: &mut Vec<String>,
) {
    let initial = provider
        .retry_initial_backoff_ms
        .unwrap_or(gateway.retry.initial_backoff_ms);
    let max = provider
        .retry_max_backoff_ms
        .unwrap_or(gateway.retry.max_backoff_ms);

    if provider.retry_initial_backoff_ms == Some(0) {
        errors.push(format!(
            "provider `{}` retry_initial_backoff_ms must be greater than 0",
            provider.name
        ));
    }
    if provider.retry_max_backoff_ms == Some(0) {
        errors.push(format!(
            "provider `{}` retry_max_backoff_ms must be greater than 0",
            provider.name
        ));
    }
    if max < initial {
        errors.push(format!(
            "provider `{}` retry_max_backoff_ms must be greater than or equal to retry_initial_backoff_ms",
            provider.name
        ));
    }
}

fn validate_provider_breaker(
    provider: &ProviderConfig,
    gateway: &GatewayConfig,
    errors: &mut Vec<String>,
) {
    if provider.circuit_breaker_failure_threshold == Some(0) {
        errors.push(format!(
            "provider `{}` circuit_breaker_failure_threshold must be greater than 0",
            provider.name
        ));
    }
    if provider.circuit_breaker_open_duration_ms == Some(0) {
        errors.push(format!(
            "provider `{}` circuit_breaker_open_duration_ms must be greater than 0",
            provider.name
        ));
    }
    if provider.circuit_breaker_half_open_max_probes == Some(0) {
        errors.push(format!(
            "provider `{}` circuit_breaker_half_open_max_probes must be greater than 0",
            provider.name
        ));
    }

    let failure_threshold = provider
        .circuit_breaker_failure_threshold
        .unwrap_or(gateway.circuit_breaker.failure_threshold);
    let open_duration = provider
        .circuit_breaker_open_duration_ms
        .unwrap_or(gateway.circuit_breaker.open_duration_ms);
    let half_open_probes = provider
        .circuit_breaker_half_open_max_probes
        .unwrap_or(gateway.circuit_breaker.half_open_max_probes);

    if failure_threshold == 0 {
        errors.push(format!(
            "provider `{}` effective circuit breaker failure threshold must be greater than 0",
            provider.name
        ));
    }
    if open_duration == 0 {
        errors.push(format!(
            "provider `{}` effective circuit breaker open duration must be greater than 0",
            provider.name
        ));
    }
    if half_open_probes == 0 {
        errors.push(format!(
            "provider `{}` effective circuit breaker half-open probes must be greater than 0",
            provider.name
        ));
    }
}
