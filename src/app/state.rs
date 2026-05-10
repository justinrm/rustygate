use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    auth::keys::{SharedKeyStore, StaticKeyStore},
    cache::response::ResponseCache,
    config::{PrefixAffinityConfig, RouteExposureConfig, RoutingPolicy},
    models::chat::ChatValidationLimits,
    providers::provider::ProviderEntry,
    rate_limit::{RateLimitBackend, RateLimiter},
    routing::{
        admission::AdmissionController,
        health::{ProviderHealthRegistry, SharedProviderHealthRegistry},
        model_pools::ModelPoolIndex,
        prefix_affinity::PrefixAffinityIndex,
        resilience::{ProviderResiliencePolicy, ResilienceRegistry},
    },
    storage::sqlite::SqliteRequestLogStore,
    telemetry::{metrics::MetricsRegistry, request_log::RequestLoggingConfig},
};

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
    pub model_pools: Arc<ModelPoolIndex>,
    pub routing_policy: RoutingPolicy,
    pub stream_idle_timeout: Duration,
    pub prefix_affinity: PrefixAffinityConfig,
    pub route_exposure: RouteExposureConfig,
    pub prefix_affinity_index: Arc<PrefixAffinityIndex>,
    pub admission: Arc<AdmissionController>,
}

const DEFAULT_GATEWAY_API_KEY: &str = "test-gateway-key";

fn default_key_store() -> SharedKeyStore {
    Arc::new(StaticKeyStore::new(DEFAULT_GATEWAY_API_KEY))
}

fn default_resilience_registry(provider_names: &[String]) -> Arc<ResilienceRegistry> {
    Arc::new(ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        std::collections::HashMap::new(),
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
            model_pools: Arc::new(ModelPoolIndex::default()),
            routing_policy: RoutingPolicy::Priority,
            stream_idle_timeout: Duration::from_millis(30_000),
            prefix_affinity: PrefixAffinityConfig::default(),
            route_exposure: RouteExposureConfig::default(),
            prefix_affinity_index: Arc::new(PrefixAffinityIndex::new(
                &PrefixAffinityConfig::default(),
            )),
            admission: AdmissionController::disabled(),
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
            model_pools: Arc::new(ModelPoolIndex::default()),
            routing_policy: RoutingPolicy::Priority,
            stream_idle_timeout: Duration::from_millis(30_000),
            prefix_affinity: PrefixAffinityConfig::default(),
            route_exposure: RouteExposureConfig::default(),
            prefix_affinity_index: Arc::new(PrefixAffinityIndex::new(
                &PrefixAffinityConfig::default(),
            )),
            admission: AdmissionController::disabled(),
        }
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
