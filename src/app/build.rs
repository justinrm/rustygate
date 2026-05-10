use std::{collections::HashMap, env, sync::Arc, time::Duration};

use crate::{
    auth::keys::{SqliteKeyStore, StaticKeyStore},
    cache::response::{MemoryResponseCache, ResponseCache, SqliteResponseCache},
    config::{
        AppConfig, CacheBackendConfig, GatewayConfig, ProviderConfig, ProviderKind,
        RateLimitBackendConfig,
    },
    models::chat::ChatValidationLimits,
    providers::{
        anthropic::AnthropicProvider,
        mock::MockProvider,
        openai_compatible::OpenAiCompatibleProvider,
        provider::{ChatProvider, ProviderEntry, ProviderPricing},
    },
    rate_limit::{RateLimitBackend, RateLimiter},
    routing::{
        admission::{AdmissionController, AdmissionLimits},
        model_pools::ModelPoolIndex,
        prefix_affinity::PrefixAffinityIndex,
        resilience::{
            CircuitBreakerPolicy, ProviderResiliencePolicy, ResilienceRegistry, RetryPolicy,
        },
    },
    storage::sqlite::{SqliteRequestLogStore, StorageError},
    telemetry::request_log::RequestLoggingConfig,
};

use super::state::AppState;

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

impl AppState {
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
        state.model_pools = Arc::new(ModelPoolIndex::from_configs(&config.model_pools));
        state.routing_policy = config.gateway.routing_policy;
        state.stream_idle_timeout = Duration::from_millis(config.gateway.stream_idle_timeout_ms);
        state.prefix_affinity = config.gateway.prefix_affinity.clone();
        state.route_exposure = config.gateway.route_exposure;
        state.prefix_affinity_index =
            Arc::new(PrefixAffinityIndex::new(&config.gateway.prefix_affinity));
        state.admission = AdmissionController::new(AdmissionLimits::from_config(
            &config.gateway.admission,
            &config.providers,
            &config.model_pools,
        ));
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
}

pub(super) fn gateway_resilience_defaults(gateway: &GatewayConfig) -> ProviderResiliencePolicy {
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

pub(super) fn provider_resilience_policy(
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
