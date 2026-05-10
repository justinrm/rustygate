mod build;
mod middleware;
mod router;
mod state;

pub use build::AppStateInitError;
pub use middleware::RequestId;
pub use router::{router, router_with_state};
pub use state::AppState;

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use super::AppState;
    use crate::{
        config::{
            AdmissionConfig, GatewayCircuitBreakerConfig, GatewayConfig, GatewayRetryConfig,
            PrefixAffinityConfig, ProviderConfig, ProviderKind, RateLimitConfig,
            RequestLimitsConfig, RouteExposureConfig, RoutingPolicy,
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
        let gateway = gateway_config();
        let provider = provider_config(
            Some(1_000),
            Some(5),
            Some(10),
            Some(100),
            Some(7),
            Some(6),
            Some(2_000),
            Some(2),
        );

        let policy = super::build::provider_resilience_policy(&provider, &gateway);

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
        let gateway = gateway_config();
        let provider = provider_config(None, None, None, None, None, None, None, None);

        let policy = super::build::provider_resilience_policy(&provider, &gateway);

        assert_eq!(policy.timeout_ms, Some(30_000));
        assert_eq!(policy.retry.max_retries, 1);
        assert_eq!(policy.retry.initial_backoff_ms, 100);
        assert_eq!(policy.retry.max_backoff_ms, 500);
        assert_eq!(policy.retry.jitter_ms, 25);
        assert_eq!(policy.breaker.failure_threshold, 3);
        assert_eq!(policy.breaker.open_duration_ms, 5_000);
        assert_eq!(policy.breaker.half_open_max_probes, 1);
    }

    fn gateway_config() -> GatewayConfig {
        GatewayConfig {
            default_timeout_ms: 30_000,
            stream_idle_timeout_ms: 30_000,
            max_retries: 1,
            health_check_interval_ms: 30_000,
            routing_policy: RoutingPolicy::Priority,
            prefix_affinity: PrefixAffinityConfig::default(),
            route_exposure: RouteExposureConfig::default(),
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
            admission: AdmissionConfig::default(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn provider_config(
        timeout_ms: Option<u64>,
        max_retries: Option<u32>,
        retry_initial_backoff_ms: Option<u64>,
        retry_max_backoff_ms: Option<u64>,
        retry_jitter_ms: Option<u64>,
        circuit_breaker_failure_threshold: Option<u32>,
        circuit_breaker_open_duration_ms: Option<u64>,
        circuit_breaker_half_open_max_probes: Option<u32>,
    ) -> ProviderConfig {
        ProviderConfig {
            name: "mock-primary".into(),
            kind: ProviderKind::Mock,
            model: "mock-fast-v1".into(),
            priority: 1,
            failure_rate: 0.0,
            base_latency_ms: 0,
            base_url: None,
            api_key_env: None,
            timeout_ms,
            max_retries,
            retry_initial_backoff_ms,
            retry_max_backoff_ms,
            retry_jitter_ms,
            circuit_breaker_failure_threshold,
            circuit_breaker_open_duration_ms,
            circuit_breaker_half_open_max_probes,
            max_in_flight: None,
            cost_per_1k_input_tokens: 0.0,
            cost_per_1k_output_tokens: 0.0,
        }
    }
}
