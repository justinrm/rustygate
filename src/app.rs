use std::{
    env,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    routing::{get, post},
    Router,
};

use crate::{
    config::{AppConfig, ProviderKind},
    providers::{
        anthropic::AnthropicProvider,
        mock::MockProvider,
        openai_compatible::OpenAiCompatibleProvider,
        provider::{ChatProvider, ProviderEntry, ProviderPricing},
    },
    routes,
    storage::sqlite::{SqliteRequestLogStore, StorageError},
    telemetry::{metrics::MetricsRegistry, request_log::RequestLoggingConfig},
};

#[derive(Debug, thiserror::Error)]
pub enum AppStateInitError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("provider configuration error for `{provider}`: {message}")]
    ProviderConfig { provider: String, message: String },
}

#[derive(Clone)]
pub struct AppState {
    pub providers: Vec<ProviderEntry>,
    pub metrics: Arc<Mutex<MetricsRegistry>>,
    pub request_logging: RequestLoggingConfig,
    pub request_log_store: Option<Arc<SqliteRequestLogStore>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            metrics: Arc::new(Mutex::new(MetricsRegistry::default())),
            request_logging: RequestLoggingConfig::default(),
            request_log_store: None,
        }
    }
}

impl AppState {
    pub fn from_providers(mut providers: Vec<ProviderEntry>) -> Self {
        providers.sort_by_key(|entry| entry.priority);
        Self {
            providers,
            metrics: Arc::new(Mutex::new(MetricsRegistry::default())),
            request_logging: RequestLoggingConfig::default(),
            request_log_store: None,
        }
    }

    pub async fn from_config(config: &AppConfig) -> Result<Self, AppStateInitError> {
        let timeout = Duration::from_millis(config.gateway.default_timeout_ms);
        let mut providers = Vec::with_capacity(config.providers.len());

        for provider in &config.providers {
            let pricing = ProviderPricing {
                cost_per_1k_input_tokens: provider.cost_per_1k_input_tokens,
                cost_per_1k_output_tokens: provider.cost_per_1k_output_tokens,
            };

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

pub fn router() -> Router {
    router_with_state(AppState::default())
}

pub fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route("/stats", get(routes::stats::stats))
        .route("/stats/providers", get(routes::stats::provider_stats))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::AppState;
    use crate::providers::{
        mock::MockProvider,
        provider::{ChatProvider, ProviderEntry, ProviderPricing},
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
}
