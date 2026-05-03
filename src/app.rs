use std::{
    collections::{BTreeMap, HashMap},
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::DefaultBodyLimit,
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use uuid::Uuid;

use crate::{
    auth::keys::{AuthenticatedKey, SharedKeyStore, SqliteKeyStore, StaticKeyStore},
    cache::response::{MemoryResponseCache, ResponseCache, SqliteResponseCache},
    config::{
        AppConfig, CacheBackendConfig, GatewayConfig, ProviderConfig, ProviderKind,
        RateLimitBackendConfig, RoutingPolicy,
    },
    error::AppError,
    models::chat::ChatValidationLimits,
    providers::{
        anthropic::AnthropicProvider,
        mock::MockProvider,
        openai_compatible::OpenAiCompatibleProvider,
        provider::{ChatProvider, ProviderEntry, ProviderPricing},
    },
    rate_limit::{RateLimitBackend, RateLimiter},
    routes,
    routing::health::{ProviderHealthRegistry, SharedProviderHealthRegistry},
    routing::resilience::{
        CircuitBreakerPolicy, ProviderResiliencePolicy, ResilienceRegistry, RetryPolicy,
    },
    storage::sqlite::{SqliteRequestLogStore, StorageError},
    telemetry::{metrics::MetricsRegistry, request_log::RequestLoggingConfig},
};

#[derive(Debug, thiserror::Error)]
pub enum AppStateInitError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Auth(#[from] crate::auth::keys::AuthError),
    #[error("gateway configuration error: {message}")]
    GatewayConfig { message: String },
    #[error("provider configuration error for `{provider}`: {message}")]
    ProviderConfig { provider: String, message: String },
}

#[derive(Clone)]
pub struct AppState {
    pub providers: Vec<ProviderEntry>,
    pub metrics: Arc<Mutex<MetricsRegistry>>,
    pub request_logging: RequestLoggingConfig,
    pub request_log_store: Option<Arc<SqliteRequestLogStore>>,
    pub response_cache: Option<Arc<dyn ResponseCache>>,
    pub response_cache_ttl: Duration,
    pub key_store: SharedKeyStore,
    pub rate_limiter: RateLimiter,
    pub rate_limit_backend: Arc<dyn RateLimitBackend>,
    pub rate_limit_backend_is_redis: bool,
    pub resilience: Arc<ResilienceRegistry>,
    pub provider_health: SharedProviderHealthRegistry,
    pub chat_validation_limits: ChatValidationLimits,
    pub max_chat_body_bytes: usize,
    pub model_aliases: Arc<BTreeMap<String, String>>,
    pub routing_policy: RoutingPolicy,
}

const DEFAULT_GATEWAY_API_KEY: &str = "test-gateway-key";

fn default_key_store() -> SharedKeyStore {
    Arc::new(StaticKeyStore::new(DEFAULT_GATEWAY_API_KEY))
}

fn default_resilience_registry(provider_names: &[String]) -> Arc<ResilienceRegistry> {
    Arc::new(ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        HashMap::new(),
        provider_names,
    ))
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            metrics: Arc::new(Mutex::new(MetricsRegistry::default())),
            request_logging: RequestLoggingConfig::default(),
            request_log_store: None,
            response_cache: None,
            response_cache_ttl: Duration::from_secs(600),
            key_store: default_key_store(),
            rate_limiter: RateLimiter::new(&Default::default()),
            rate_limit_backend: Arc::new(RateLimiter::new(&Default::default())),
            rate_limit_backend_is_redis: false,
            resilience: default_resilience_registry(&[]),
            provider_health: Arc::new(ProviderHealthRegistry::new(&[])),
            chat_validation_limits: ChatValidationLimits::default(),
            max_chat_body_bytes: 65_536,
            model_aliases: Arc::new(BTreeMap::new()),
            routing_policy: RoutingPolicy::Priority,
        }
    }
}

impl AppState {
    pub fn from_providers(mut providers: Vec<ProviderEntry>) -> Self {
        providers.sort_by_key(|entry| entry.priority);
        Self {
            provider_health: Arc::new(ProviderHealthRegistry::new(
                &providers
                    .iter()
                    .map(|entry| entry.provider.name().to_string())
                    .collect::<Vec<_>>(),
            )),
            resilience: default_resilience_registry(
                &providers
                    .iter()
                    .map(|entry| entry.provider.name().to_string())
                    .collect::<Vec<_>>(),
            ),
            providers,
            metrics: Arc::new(Mutex::new(MetricsRegistry::default())),
            request_logging: RequestLoggingConfig::default(),
            request_log_store: None,
            response_cache: None,
            response_cache_ttl: Duration::from_secs(600),
            key_store: default_key_store(),
            rate_limiter: RateLimiter::new(&Default::default()),
            rate_limit_backend: Arc::new(RateLimiter::new(&Default::default())),
            rate_limit_backend_is_redis: false,
            chat_validation_limits: ChatValidationLimits::default(),
            max_chat_body_bytes: 65_536,
            model_aliases: Arc::new(BTreeMap::new()),
            routing_policy: RoutingPolicy::Priority,
        }
    }

    pub async fn from_config(config: &AppConfig) -> Result<Self, AppStateInitError> {
        let mut providers = Vec::with_capacity(config.providers.len());
        let mut provider_policies = HashMap::with_capacity(config.providers.len());

        for provider in &config.providers {
            let provider_policy = provider_resilience_policy(provider, &config.gateway);
            provider_policies.insert(provider.name.clone(), provider_policy);

            let pricing = ProviderPricing {
                cost_per_1k_input_tokens: provider.cost_per_1k_input_tokens,
                cost_per_1k_output_tokens: provider.cost_per_1k_output_tokens,
            };
            let timeout = Duration::from_millis(
                provider
                    .timeout_ms
                    .unwrap_or(config.gateway.default_timeout_ms),
            );

            let provider_impl: Arc<dyn ChatProvider> = match provider.kind {
                ProviderKind::Mock => Arc::new(MockProvider {
                    name: provider.name.clone(),
                    model: provider.model.clone(),
                    failure_rate: provider.failure_rate,
                    base_latency_ms: provider.base_latency_ms,
                }),
                ProviderKind::OpenaiCompatible => {
                    let api_key_env = provider.api_key_env.clone().ok_or_else(|| {
                        AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: "missing `api_key_env` for openai_compatible provider".into(),
                        }
                    })?;
                    let api_key = env::var(&api_key_env).map_err(|_| {
                        AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: format!(
                                "environment variable `{api_key_env}` is not set for provider API key"
                            ),
                        }
                    })?;
                    let base_url = provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                    let client = reqwest::Client::builder()
                        .timeout(timeout)
                        .build()
                        .map_err(|error| AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: format!("failed to build HTTP client: {error}"),
                        })?;
                    Arc::new(OpenAiCompatibleProvider {
                        name: provider.name.clone(),
                        model: provider.model.clone(),
                        base_url,
                        api_key,
                        client,
                    })
                }
                ProviderKind::Anthropic => {
                    let api_key_env = provider.api_key_env.clone().ok_or_else(|| {
                        AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: "missing `api_key_env` for anthropic provider".into(),
                        }
                    })?;
                    let api_key = env::var(&api_key_env).map_err(|_| {
                        AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: format!(
                                "environment variable `{api_key_env}` is not set for provider API key"
                            ),
                        }
                    })?;
                    let base_url = provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
                    let client = reqwest::Client::builder()
                        .timeout(timeout)
                        .build()
                        .map_err(|error| AppStateInitError::ProviderConfig {
                            provider: provider.name.clone(),
                            message: format!("failed to build HTTP client: {error}"),
                        })?;
                    Arc::new(AnthropicProvider {
                        name: provider.name.clone(),
                        model: provider.model.clone(),
                        base_url,
                        api_key,
                        client,
                    })
                }
            };

            providers.push(ProviderEntry {
                priority: provider.priority,
                provider: provider_impl,
                pricing,
            });
        }

        let mut state =
            Self::from_providers(providers).with_request_logging_config(RequestLoggingConfig {
                enabled: config.gateway.enable_request_logging,
                log_prompt_content: config.gateway.log_prompt_content,
            });
        state.key_store = if config.storage.enabled {
            Arc::new(SqliteKeyStore::connect(&config.storage.database_url).await?)
        } else {
            let gateway_api_key = env::var(&config.gateway.api_key_env).map_err(|_| {
                AppStateInitError::GatewayConfig {
                    message: format!(
                        "environment variable `{}` is not set for gateway API key",
                        config.gateway.api_key_env
                    ),
                }
            })?;
            if gateway_api_key.trim().is_empty() {
                return Err(AppStateInitError::GatewayConfig {
                    message: format!(
                        "environment variable `{}` for gateway API key must not be empty",
                        config.gateway.api_key_env
                    ),
                });
            }
            Arc::new(StaticKeyStore::new(gateway_api_key))
        };
        state.rate_limiter = RateLimiter::new(&config.gateway.rate_limit);
        state.rate_limit_backend =
            build_rate_limit_backend(&config.gateway.rate_limit, state.rate_limiter.clone())
                .await?;
        state.rate_limit_backend_is_redis =
            config.gateway.rate_limit.backend == RateLimitBackendConfig::Redis;
        state.chat_validation_limits = ChatValidationLimits {
            max_messages_per_request: config.gateway.request_limits.max_messages_per_request,
            max_message_content_chars: config.gateway.request_limits.max_message_content_chars,
        };
        state.max_chat_body_bytes = config.gateway.request_limits.max_chat_body_bytes;
        state.model_aliases = Arc::new(config.gateway.model_aliases.clone());
        state.routing_policy = config.gateway.routing_policy;
        if config.cache.enabled {
            state.response_cache = Some(build_response_cache(config).await?);
            state.response_cache_ttl = Duration::from_secs(config.cache.default_ttl_seconds);
        }
        state.resilience = Arc::new(ResilienceRegistry::new(
            gateway_resilience_defaults(&config.gateway),
            provider_policies,
            &state.provider_names(),
        ));

        if config.storage.enabled {
            let store = SqliteRequestLogStore::connect(&config.storage.database_url).await?;
            state = state.with_request_log_store(Arc::new(store));
        }

        Ok(state)
    }

    pub fn with_request_logging_config(mut self, request_logging: RequestLoggingConfig) -> Self {
        self.request_logging = request_logging;
        self
    }

    pub fn with_request_log_store(mut self, store: Arc<SqliteRequestLogStore>) -> Self {
        self.request_log_store = Some(store);
        self
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|entry| entry.provider.name().to_owned())
            .collect()
    }
}

fn gateway_resilience_defaults(gateway: &GatewayConfig) -> ProviderResiliencePolicy {
    ProviderResiliencePolicy {
        timeout_ms: Some(gateway.default_timeout_ms),
        retry: RetryPolicy {
            max_retries: gateway.max_retries,
            initial_backoff_ms: gateway.retry.initial_backoff_ms,
            max_backoff_ms: gateway.retry.max_backoff_ms,
            jitter_ms: gateway.retry.jitter_ms,
        },
        breaker: CircuitBreakerPolicy {
            failure_threshold: gateway.circuit_breaker.failure_threshold,
            open_duration_ms: gateway.circuit_breaker.open_duration_ms,
            half_open_max_probes: gateway.circuit_breaker.half_open_max_probes,
        },
    }
}

fn provider_resilience_policy(
    provider: &ProviderConfig,
    gateway: &GatewayConfig,
) -> ProviderResiliencePolicy {
    ProviderResiliencePolicy {
        timeout_ms: provider.timeout_ms.or(Some(gateway.default_timeout_ms)),
        retry: RetryPolicy {
            max_retries: provider.max_retries.unwrap_or(gateway.max_retries),
            initial_backoff_ms: provider
                .retry_initial_backoff_ms
                .unwrap_or(gateway.retry.initial_backoff_ms),
            max_backoff_ms: provider
                .retry_max_backoff_ms
                .unwrap_or(gateway.retry.max_backoff_ms),
            jitter_ms: provider.retry_jitter_ms.unwrap_or(gateway.retry.jitter_ms),
        },
        breaker: CircuitBreakerPolicy {
            failure_threshold: provider
                .circuit_breaker_failure_threshold
                .unwrap_or(gateway.circuit_breaker.failure_threshold),
            open_duration_ms: provider
                .circuit_breaker_open_duration_ms
                .unwrap_or(gateway.circuit_breaker.open_duration_ms),
            half_open_max_probes: provider
                .circuit_breaker_half_open_max_probes
                .unwrap_or(gateway.circuit_breaker.half_open_max_probes),
        },
    }
}

async fn build_response_cache(
    config: &AppConfig,
) -> Result<Arc<dyn ResponseCache>, AppStateInitError> {
    match config.cache.backend {
        CacheBackendConfig::Memory => Ok(Arc::new(MemoryResponseCache::new(
            Duration::from_secs(config.cache.default_ttl_seconds),
            config.cache.max_entries,
        ))),
        CacheBackendConfig::Sqlite => {
            let store = SqliteKeyStore::connect(&config.storage.database_url).await?;
            let pool = store.pool().clone();
            Ok(Arc::new(SqliteResponseCache::new(pool)))
        }
    }
}

async fn build_rate_limit_backend(
    config: &crate::config::RateLimitConfig,
    local: RateLimiter,
) -> Result<Arc<dyn RateLimitBackend>, AppStateInitError> {
    match config.backend {
        RateLimitBackendConfig::Local => Ok(Arc::new(local)),
        RateLimitBackendConfig::Redis => build_redis_rate_limit_backend(config, local).await,
    }
}

#[cfg(feature = "redis-backend")]
async fn build_redis_rate_limit_backend(
    config: &crate::config::RateLimitConfig,
    local: RateLimiter,
) -> Result<Arc<dyn RateLimitBackend>, AppStateInitError> {
    let env_name =
        config
            .redis_url_env
            .as_deref()
            .ok_or_else(|| AppStateInitError::GatewayConfig {
                message: "gateway.rate_limit.redis_url_env is required for redis backend".into(),
            })?;
    let redis_url = env::var(env_name).map_err(|_| AppStateInitError::GatewayConfig {
        message: format!("environment variable `{env_name}` is not set for Redis rate limiting"),
    })?;
    let fallback = config.redis_fallback_to_local.then_some(local);
    let backend = crate::rate_limit::redis_backend::RedisRateLimitBackend::connect(
        &redis_url, config, fallback,
    )
    .await
    .map_err(|error| AppStateInitError::GatewayConfig {
        message: format!("failed to connect Redis rate-limit backend: {error}"),
    })?;
    Ok(Arc::new(backend))
}

#[cfg(not(feature = "redis-backend"))]
async fn build_redis_rate_limit_backend(
    _config: &crate::config::RateLimitConfig,
    _local: RateLimiter,
) -> Result<Arc<dyn RateLimitBackend>, AppStateInitError> {
    Err(AppStateInitError::GatewayConfig {
        message: "redis rate-limit backend requires the `redis-backend` feature".into(),
    })
}

pub fn router() -> Router {
    router_with_state(AppState::default())
}

pub fn router_with_state(mut state: AppState) -> Router {
    if !state.rate_limit_backend_is_redis {
        state.rate_limit_backend = Arc::new(state.rate_limiter.clone());
    }
    let max_chat_body_bytes = state.max_chat_body_bytes;
    let protected_routes = Router::new()
        .route("/v1/responses", post(routes::compat::responses))
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route("/v1/embeddings", post(routes::compat::embeddings))
        .route("/v1/moderations", post(routes::compat::moderations))
        .route(
            "/v1/images/generations",
            post(routes::compat::image_generation),
        )
        .route("/v1/images/edits", post(routes::compat::image_edit))
        .route(
            "/v1/images/variations",
            post(routes::compat::image_variation),
        )
        .route(
            "/v1/audio/transcriptions",
            post(routes::compat::audio_transcription),
        )
        .route(
            "/v1/audio/translations",
            post(routes::compat::audio_translation),
        )
        .route(
            "/v1/files",
            get(routes::compat::list_files).post(routes::compat::create_file),
        )
        .route(
            "/v1/files/{file_id}",
            get(routes::compat::retrieve_file).delete(routes::compat::delete_file),
        )
        .route(
            "/v1/files/{file_id}/content",
            get(routes::compat::file_content),
        )
        .route(
            "/v1/batches",
            get(routes::compat::list_batches).post(routes::compat::create_batch),
        )
        .route(
            "/v1/batches/{batch_id}",
            get(routes::compat::retrieve_batch),
        )
        .route(
            "/v1/batches/{batch_id}/cancel",
            post(routes::compat::cancel_batch),
        )
        .route(
            "/v1/fine_tuning/jobs",
            get(routes::compat::list_fine_tuning_jobs).post(routes::compat::create_fine_tuning_job),
        )
        .route(
            "/v1/fine_tuning/jobs/{job_id}",
            get(routes::compat::retrieve_fine_tuning_job),
        )
        .route(
            "/v1/fine_tuning/jobs/{job_id}/cancel",
            post(routes::compat::cancel_fine_tuning_job),
        )
        .route(
            "/v1/fine_tuning/jobs/{job_id}/events",
            get(routes::compat::list_fine_tuning_events),
        )
        .route(
            "/v1/realtime/sessions",
            post(routes::compat::realtime_session),
        )
        .route("/v1/models", get(routes::models::list_models))
        .route("/stats", get(routes::stats::stats))
        .route("/stats/providers", get(routes::stats::provider_stats))
        .route("/metrics", get(routes::stats::prometheus_metrics))
        .layer(DefaultBodyLimit::max(max_chat_body_bytes))
        .layer(from_fn_with_state(
            state.clone(),
            per_key_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(state.clone(), auth_middleware))
        .layer(from_fn_with_state(
            state.clone(),
            pre_auth_rate_limit_middleware,
        ));

    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .merge(protected_routes)
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(api_key) = bearer_token(&request) else {
        return AppError::Unauthorized {
            message: "missing bearer token".into(),
            request_id: Some(Uuid::new_v4()),
        }
        .into_response();
    };
    let authenticated_key = match state.key_store.authenticate(api_key).await {
        Ok(Some(key)) => key,
        Ok(None) | Err(_) => {
            return AppError::Unauthorized {
                message: "invalid bearer token".into(),
                request_id: Some(Uuid::new_v4()),
            }
            .into_response();
        }
    };

    if !role_allows_path(authenticated_key.role, request.uri().path()) {
        return AppError::Forbidden {
            message: "API key is not allowed to access this route".into(),
            request_id: Some(Uuid::new_v4()),
        }
        .into_response();
    }

    request.extensions_mut().insert(authenticated_key);
    next.run(request).await
}

async fn pre_auth_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(retry_after_seconds) = state.rate_limit_backend.check_global().await {
        return AppError::GatewayRateLimited {
            request_id: Some(Uuid::new_v4()),
            retry_after_seconds,
        }
        .into_response();
    }

    next.run(request).await
}

async fn per_key_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(api_key) = request.extensions().get::<AuthenticatedKey>().cloned() else {
        return AppError::Unauthorized {
            message: "missing authenticated API key".into(),
            request_id: Some(Uuid::new_v4()),
        }
        .into_response();
    };

    if let Err(quota) = state.key_store.check_quota(&api_key).await {
        return AppError::GatewayRateLimited {
            request_id: Some(Uuid::new_v4()),
            retry_after_seconds: quota.retry_after_seconds,
        }
        .into_response();
    }

    let burst = api_key
        .limits
        .requests_per_minute
        .map(|rpm| (rpm / 4).max(1));
    if let Err(retry_after_seconds) = state
        .rate_limit_backend
        .check_key(&api_key.id, api_key.limits.requests_per_minute, burst)
        .await
    {
        return AppError::GatewayRateLimited {
            request_id: Some(Uuid::new_v4()),
            retry_after_seconds,
        }
        .into_response();
    }

    next.run(request).await
}

fn bearer_token(request: &Request) -> Option<&str> {
    let header = request.headers().get(AUTHORIZATION)?;
    let header = header.to_str().ok()?;
    let token = header.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }
    Some(token)
}

fn role_allows_path(role: crate::auth::keys::KeyRole, path: &str) -> bool {
    if path == "/stats" || path == "/stats/providers" || path == "/metrics" {
        role.allows_observability()
    } else {
        role.allows_inference() || path == "/v1/models"
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::AppState;
    use crate::{
        config::{
            GatewayCircuitBreakerConfig, GatewayConfig, GatewayRetryConfig, ProviderConfig,
            ProviderKind, RateLimitConfig, RequestLimitsConfig, RoutingPolicy,
        },
        providers::{
            mock::MockProvider,
            provider::{ChatProvider, ProviderEntry, ProviderPricing},
        },
    };

    #[test]
    fn provider_names_follow_priority_order() {
        let providers = vec![
            ProviderEntry {
                priority: 3,
                provider: Arc::new(MockProvider::new("mock-third", "mock-v1"))
                    as Arc<dyn ChatProvider>,
                pricing: ProviderPricing::default(),
            },
            ProviderEntry {
                priority: 1,
                provider: Arc::new(MockProvider::new("mock-first", "mock-v1"))
                    as Arc<dyn ChatProvider>,
                pricing: ProviderPricing::default(),
            },
            ProviderEntry {
                priority: 2,
                provider: Arc::new(MockProvider::new("mock-second", "mock-v1"))
                    as Arc<dyn ChatProvider>,
                pricing: ProviderPricing::default(),
            },
        ];

        let state = AppState::from_providers(providers);

        assert_eq!(
            state.provider_names(),
            vec![
                "mock-first".to_string(),
                "mock-second".to_string(),
                "mock-third".to_string(),
            ]
        );
    }

    #[test]
    fn provider_resilience_policy_uses_provider_overrides() {
        let gateway = GatewayConfig {
            default_timeout_ms: 30_000,
            max_retries: 1,
            health_check_interval_ms: 30_000,
            routing_policy: RoutingPolicy::Priority,
            model_aliases: BTreeMap::new(),
            retry: GatewayRetryConfig {
                initial_backoff_ms: 100,
                max_backoff_ms: 500,
                jitter_ms: 25,
            },
            circuit_breaker: GatewayCircuitBreakerConfig {
                failure_threshold: 3,
                open_duration_ms: 5_000,
                half_open_max_probes: 1,
            },
            enable_request_logging: true,
            log_prompt_content: false,
            api_key_env: "RUSTYGATE_GATEWAY_API_KEY".into(),
            rate_limit: RateLimitConfig::default(),
            request_limits: RequestLimitsConfig::default(),
        };
        let provider = ProviderConfig {
            name: "mock-primary".into(),
            kind: ProviderKind::Mock,
            model: "mock-fast-v1".into(),
            priority: 1,
            failure_rate: 0.0,
            base_latency_ms: 0,
            base_url: None,
            api_key_env: None,
            timeout_ms: Some(1_000),
            max_retries: Some(5),
            retry_initial_backoff_ms: Some(10),
            retry_max_backoff_ms: Some(100),
            retry_jitter_ms: Some(7),
            circuit_breaker_failure_threshold: Some(6),
            circuit_breaker_open_duration_ms: Some(2_000),
            circuit_breaker_half_open_max_probes: Some(2),
            cost_per_1k_input_tokens: 0.0,
            cost_per_1k_output_tokens: 0.0,
        };

        let policy = super::provider_resilience_policy(&provider, &gateway);

        assert_eq!(policy.timeout_ms, Some(1_000));
        assert_eq!(policy.retry.max_retries, 5);
        assert_eq!(policy.retry.initial_backoff_ms, 10);
        assert_eq!(policy.retry.max_backoff_ms, 100);
        assert_eq!(policy.retry.jitter_ms, 7);
        assert_eq!(policy.breaker.failure_threshold, 6);
        assert_eq!(policy.breaker.open_duration_ms, 2_000);
        assert_eq!(policy.breaker.half_open_max_probes, 2);
    }

    #[test]
    fn provider_resilience_policy_falls_back_to_gateway_defaults() {
        let gateway = GatewayConfig {
            default_timeout_ms: 20_000,
            max_retries: 2,
            health_check_interval_ms: 30_000,
            routing_policy: RoutingPolicy::Priority,
            model_aliases: BTreeMap::new(),
            retry: GatewayRetryConfig {
                initial_backoff_ms: 50,
                max_backoff_ms: 500,
                jitter_ms: 11,
            },
            circuit_breaker: GatewayCircuitBreakerConfig {
                failure_threshold: 4,
                open_duration_ms: 9_000,
                half_open_max_probes: 3,
            },
            enable_request_logging: true,
            log_prompt_content: false,
            api_key_env: "RUSTYGATE_GATEWAY_API_KEY".into(),
            rate_limit: RateLimitConfig::default(),
            request_limits: RequestLimitsConfig::default(),
        };
        let provider = ProviderConfig {
            name: "mock-primary".into(),
            kind: ProviderKind::Mock,
            model: "mock-fast-v1".into(),
            priority: 1,
            failure_rate: 0.0,
            base_latency_ms: 0,
            base_url: None,
            api_key_env: None,
            timeout_ms: None,
            max_retries: None,
            retry_initial_backoff_ms: None,
            retry_max_backoff_ms: None,
            retry_jitter_ms: None,
            circuit_breaker_failure_threshold: None,
            circuit_breaker_open_duration_ms: None,
            circuit_breaker_half_open_max_probes: None,
            cost_per_1k_input_tokens: 0.0,
            cost_per_1k_output_tokens: 0.0,
        };

        let policy = super::provider_resilience_policy(&provider, &gateway);

        assert_eq!(policy.timeout_ms, Some(20_000));
        assert_eq!(policy.retry.max_retries, 2);
        assert_eq!(policy.retry.initial_backoff_ms, 50);
        assert_eq!(policy.retry.max_backoff_ms, 500);
        assert_eq!(policy.retry.jitter_ms, 11);
        assert_eq!(policy.breaker.failure_threshold, 4);
        assert_eq!(policy.breaker.open_duration_ms, 9_000);
        assert_eq!(policy.breaker.half_open_max_probes, 3);
    }
}
