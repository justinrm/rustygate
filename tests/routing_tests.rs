use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use rustygate::{
    config::{ModelPoolConfig, PrefixAffinityConfig, RoutingPolicy},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderError, ProviderPricing},
    },
    routing::{
        fallback::{fallback_decision, RetryDecision},
        model_pools::ModelPoolIndex,
        prefix_affinity::PrefixAffinityIndex,
        prefix_fingerprint::{PrefixFingerprintConfidence, PrefixFingerprintResult},
        resilience::{CircuitBreakerPolicy, ProviderResiliencePolicy, ResilienceRegistry},
        strategy::{
            candidate_providers, candidate_providers_with_affinity, resolve_model_alias,
            select_provider, PrefixAffinityRouting,
        },
    },
    telemetry::metrics::MetricsSnapshot,
};

mod common;

use common::mock_provider_entry;

fn provider_entry(name: &str, model: &str, priority: u32) -> ProviderEntry {
    mock_provider_entry(name, model, priority)
}

fn no_model_pools() -> ModelPoolIndex {
    ModelPoolIndex::default()
}

fn prefix_affinity_config() -> PrefixAffinityConfig {
    PrefixAffinityConfig {
        ttl_seconds: 60,
        max_entries: 100,
        load_imbalance_threshold: 1,
        fallback_policy: RoutingPolicy::Priority,
    }
}

fn high_confidence_fingerprint(value: &str) -> PrefixFingerprintResult {
    PrefixFingerprintResult {
        fingerprint: Some(value.to_string()),
        confidence: PrefixFingerprintConfidence::High,
        prefix_char_length: 128,
        prefix_token_estimate: 32,
    }
}

fn low_confidence_fingerprint() -> PrefixFingerprintResult {
    PrefixFingerprintResult {
        fingerprint: None,
        confidence: PrefixFingerprintConfidence::Low,
        prefix_char_length: 0,
        prefix_token_estimate: 0,
    }
}

fn prefix_pool() -> ModelPoolIndex {
    ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "shared-model".into(),
        aliases: vec![],
        routing_policy: Some(RoutingPolicy::PrefixAffinity),
        members: vec!["replica-a".into(), "replica-b".into()],
        max_in_flight: None,
    }])
}

fn provider_names(providers: &[&ProviderEntry]) -> Vec<String> {
    providers
        .iter()
        .map(|entry| entry.provider.name().to_string())
        .collect()
}

#[test]
fn select_provider_matches_requested_model() {
    let providers = vec![
        provider_entry("mock-fast", "mock-fast-v1", 3),
        provider_entry("mock-smart", "mock-smart-v1", 1),
    ];

    let pools = no_model_pools();
    let selected =
        select_provider(&providers, Some("mock-fast-v1"), &pools).expect("provider exists");

    assert_eq!(selected.provider.name(), "mock-fast");
}

#[test]
fn select_provider_prefers_lowest_priority_when_multiple_support_requested_model() {
    let providers = vec![
        provider_entry("mock-secondary", "mock-fast-v1", 5),
        provider_entry("mock-primary", "mock-fast-v1", 1),
        provider_entry("mock-tertiary", "mock-fast-v1", 3),
    ];

    let pools = no_model_pools();
    let selected =
        select_provider(&providers, Some("mock-fast-v1"), &pools).expect("provider exists");

    assert_eq!(selected.provider.name(), "mock-primary");
}

#[test]
fn select_provider_without_requested_model_returns_none() {
    let providers = vec![
        provider_entry("mock-third", "mock-third-v1", 3),
        provider_entry("mock-first", "mock-first-v1", 1),
        provider_entry("mock-second", "mock-second-v1", 2),
    ];

    let pools = no_model_pools();
    let selected = select_provider(&providers, None, &pools);

    assert!(selected.is_none());
}

#[test]
fn select_provider_returns_none_for_unsupported_requested_model() {
    let providers = vec![
        provider_entry("mock-fast", "mock-fast-v1", 1),
        provider_entry("mock-smart", "mock-smart-v1", 2),
    ];

    let pools = no_model_pools();
    let selected = select_provider(&providers, Some("unknown-model"), &pools);

    assert!(selected.is_none());
}

#[test]
fn candidate_providers_returns_all_supported_providers_by_priority() {
    let providers = vec![
        provider_entry("mock-tertiary", "mock-fast-v1", 3),
        provider_entry("mock-primary", "mock-fast-v1", 1),
        provider_entry("mock-unrelated", "mock-smart-v1", 0),
        provider_entry("mock-secondary", "mock-fast-v1", 2),
    ];

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Priority,
        None,
        &no_model_pools(),
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(
        selected_names,
        vec!["mock-primary", "mock-secondary", "mock-tertiary"]
    );
}

#[test]
fn resolve_model_alias_maps_public_model_to_provider_model() {
    let mut aliases = BTreeMap::new();
    aliases.insert("gpt-4o".to_string(), "gpt-4o-mini".to_string());

    assert_eq!(resolve_model_alias(&aliases, "gpt-4o"), "gpt-4o-mini");
    assert_eq!(
        resolve_model_alias(&aliases, "mock-fast-v1"),
        "mock-fast-v1"
    );
}

#[test]
fn candidate_providers_can_order_by_lowest_cost() {
    let providers = vec![
        ProviderEntry {
            priority: 1,
            provider: Arc::new(MockProvider::new("mock-expensive", "mock-fast-v1")),
            pricing: ProviderPricing {
                cost_per_1k_input_tokens: 1.0,
                cost_per_1k_output_tokens: 1.0,
            },
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-cheap", "mock-fast-v1")),
            pricing: ProviderPricing {
                cost_per_1k_input_tokens: 0.1,
                cost_per_1k_output_tokens: 0.2,
            },
        },
    ];

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Cost,
        None,
        &no_model_pools(),
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["mock-cheap", "mock-expensive"]);
}

#[test]
fn candidate_providers_can_order_by_recent_latency() {
    let providers = vec![
        provider_entry("mock-slow", "mock-fast-v1", 1),
        provider_entry("mock-fast", "mock-fast-v1", 2),
    ];
    let mut snapshot = MetricsSnapshot::default();
    snapshot
        .avg_latency_ms_by_provider
        .insert("mock-slow".into(), 100.0);
    snapshot
        .avg_latency_ms_by_provider
        .insert("mock-fast".into(), 10.0);

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Latency,
        Some(&snapshot),
        &no_model_pools(),
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["mock-fast", "mock-slow"]);
}

#[test]
fn latency_routing_prefers_bounded_p95_over_all_time_average() {
    let providers = vec![
        provider_entry("mock-bursty", "mock-fast-v1", 1),
        provider_entry("mock-steady", "mock-fast-v1", 2),
    ];
    let mut snapshot = MetricsSnapshot::default();
    snapshot
        .avg_latency_ms_by_provider
        .insert("mock-bursty".into(), 10.0);
    snapshot
        .p95_latency_ms_by_provider
        .insert("mock-bursty".into(), 200.0);
    snapshot
        .avg_latency_ms_by_provider
        .insert("mock-steady".into(), 50.0);
    snapshot
        .p95_latency_ms_by_provider
        .insert("mock-steady".into(), 60.0);

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Latency,
        Some(&snapshot),
        &no_model_pools(),
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["mock-steady", "mock-bursty"]);
}

#[test]
fn latency_routing_penalizes_in_flight_load_and_recent_errors() {
    let providers = vec![
        provider_entry("replica-busy", "mock-fast-v1", 1),
        provider_entry("replica-clean", "mock-fast-v1", 2),
    ];
    let mut snapshot = MetricsSnapshot::default();
    snapshot
        .p95_latency_ms_by_provider
        .insert("replica-busy".into(), 10.0);
    snapshot
        .p95_latency_ms_by_provider
        .insert("replica-clean".into(), 20.0);
    snapshot
        .in_flight_requests_by_provider
        .insert("replica-busy".into(), 1);
    snapshot
        .recent_provider_errors_by_provider_and_category
        .entry("replica-busy".into())
        .or_default()
        .insert("timeout".into(), 1);

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Latency,
        Some(&snapshot),
        &no_model_pools(),
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["replica-clean", "replica-busy"]);
}

#[test]
fn latency_routing_penalizes_open_circuits_before_fallback_skips() {
    let providers = vec![
        provider_entry("replica-open", "mock-fast-v1", 1),
        provider_entry("replica-closed", "mock-fast-v1", 2),
    ];
    let mut snapshot = MetricsSnapshot::default();
    snapshot
        .p95_latency_ms_by_provider
        .insert("replica-open".into(), 10.0);
    snapshot
        .p95_latency_ms_by_provider
        .insert("replica-closed".into(), 20.0);
    let mut provider_policies = HashMap::new();
    provider_policies.insert(
        "replica-open".to_string(),
        ProviderResiliencePolicy {
            breaker: CircuitBreakerPolicy {
                failure_threshold: 1,
                open_duration_ms: 60_000,
                half_open_max_probes: 1,
            },
            ..ProviderResiliencePolicy::default()
        },
    );
    let resilience = ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        provider_policies,
        &["replica-open".to_string(), "replica-closed".to_string()],
    );
    resilience.record_failure("replica-open");

    let selected = candidate_providers(
        &providers,
        Some("mock-fast-v1"),
        RoutingPolicy::Latency,
        Some(&snapshot),
        &no_model_pools(),
        Some(&resilience),
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["replica-closed", "replica-open"]);
}

#[test]
fn candidate_providers_selects_pool_members_for_public_model() {
    let providers = vec![
        provider_entry("mock-a", "internal-replica-a", 2),
        provider_entry("mock-b", "internal-replica-b", 1),
        provider_entry("mock-c", "internal-replica-c", 3),
    ];
    let pools = ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "shared-model".into(),
        aliases: vec!["shared-model-v1".into()],
        routing_policy: None,
        members: vec!["mock-a".into(), "mock-b".into()],
        max_in_flight: None,
    }]);

    let selected = candidate_providers(
        &providers,
        Some("shared-model-v1"),
        RoutingPolicy::Priority,
        None,
        &pools,
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["mock-b", "mock-a"]);
}

#[test]
fn pool_routing_policy_override_applies_when_configured() {
    let providers = vec![
        ProviderEntry {
            priority: 1,
            provider: Arc::new(MockProvider::new("mock-expensive", "internal-replica-a")),
            pricing: ProviderPricing {
                cost_per_1k_input_tokens: 1.0,
                cost_per_1k_output_tokens: 1.0,
            },
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-cheap", "internal-replica-b")),
            pricing: ProviderPricing {
                cost_per_1k_input_tokens: 0.1,
                cost_per_1k_output_tokens: 0.2,
            },
        },
    ];
    let pools = ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "shared-model".into(),
        aliases: vec![],
        routing_policy: Some(RoutingPolicy::Cost),
        members: vec!["mock-expensive".into(), "mock-cheap".into()],
        max_in_flight: None,
    }]);

    let selected = candidate_providers(
        &providers,
        Some("shared-model"),
        RoutingPolicy::Priority,
        None,
        &pools,
        None,
    );
    let selected_names = selected
        .iter()
        .map(|entry| entry.provider.name())
        .collect::<Vec<_>>();

    assert_eq!(selected_names, vec!["mock-cheap", "mock-expensive"]);
}

#[test]
fn prefix_affinity_sticks_to_previous_healthy_replica() {
    let providers = vec![
        provider_entry("replica-a", "internal-replica-a", 1),
        provider_entry("replica-b", "internal-replica-b", 2),
    ];
    let pools = prefix_pool();
    let config = prefix_affinity_config();
    let index = PrefixAffinityIndex::new(&config);
    index.record("prefix-a", "replica-b");
    let fingerprint = high_confidence_fingerprint("prefix-a");

    let selection = candidate_providers_with_affinity(
        &providers,
        Some("shared-model"),
        RoutingPolicy::Priority,
        None,
        &pools,
        None,
        Some(PrefixAffinityRouting {
            index: &index,
            config: &config,
            fingerprint: &fingerprint,
        }),
    );

    assert_eq!(selection.effective_policy, RoutingPolicy::PrefixAffinity);
    assert_eq!(selection.reason, "prefix_hit");
    assert_eq!(
        provider_names(&selection.candidates),
        vec!["replica-b", "replica-a"]
    );
}

#[test]
fn prefix_affinity_distributes_different_prefixes_across_replicas() {
    let providers = vec![
        provider_entry("replica-a", "internal-replica-a", 1),
        provider_entry("replica-b", "internal-replica-b", 2),
    ];
    let pools = prefix_pool();
    let config = prefix_affinity_config();
    let index = PrefixAffinityIndex::new(&config);
    let mut selected_replicas = BTreeMap::new();

    for prefix_index in 0..20 {
        let fingerprint = high_confidence_fingerprint(&format!("prefix-{prefix_index}"));
        let selection = candidate_providers_with_affinity(
            &providers,
            Some("shared-model"),
            RoutingPolicy::Priority,
            None,
            &pools,
            None,
            Some(PrefixAffinityRouting {
                index: &index,
                config: &config,
                fingerprint: &fingerprint,
            }),
        );
        selected_replicas.insert(selection.candidates[0].provider.name().to_string(), true);
    }

    assert!(selected_replicas.contains_key("replica-a"));
    assert!(selected_replicas.contains_key("replica-b"));
}

#[test]
fn prefix_affinity_load_imbalance_overrides_previous_replica() {
    let providers = vec![
        provider_entry("replica-a", "internal-replica-a", 1),
        provider_entry("replica-b", "internal-replica-b", 2),
    ];
    let pools = prefix_pool();
    let config = prefix_affinity_config();
    let index = PrefixAffinityIndex::new(&config);
    index.record("prefix-a", "replica-a");
    let fingerprint = high_confidence_fingerprint("prefix-a");
    let mut snapshot = MetricsSnapshot::default();
    snapshot
        .in_flight_requests_by_provider
        .insert("replica-a".into(), 4);

    let selection = candidate_providers_with_affinity(
        &providers,
        Some("shared-model"),
        RoutingPolicy::Priority,
        Some(&snapshot),
        &pools,
        None,
        Some(PrefixAffinityRouting {
            index: &index,
            config: &config,
            fingerprint: &fingerprint,
        }),
    );

    assert_eq!(selection.reason, "load_imbalanced");
    assert_eq!(
        provider_names(&selection.candidates),
        vec!["replica-b", "replica-a"]
    );
}

#[test]
fn prefix_affinity_open_circuit_overrides_previous_replica() {
    let providers = vec![
        provider_entry("replica-a", "internal-replica-a", 1),
        provider_entry("replica-b", "internal-replica-b", 2),
    ];
    let pools = prefix_pool();
    let config = prefix_affinity_config();
    let index = PrefixAffinityIndex::new(&config);
    index.record("prefix-a", "replica-a");
    let fingerprint = high_confidence_fingerprint("prefix-a");
    let mut provider_policies = HashMap::new();
    provider_policies.insert(
        "replica-a".to_string(),
        ProviderResiliencePolicy {
            breaker: CircuitBreakerPolicy {
                failure_threshold: 1,
                open_duration_ms: 60_000,
                half_open_max_probes: 1,
            },
            ..ProviderResiliencePolicy::default()
        },
    );
    let resilience = ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        provider_policies,
        &["replica-a".to_string(), "replica-b".to_string()],
    );
    resilience.record_failure("replica-a");

    let selection = candidate_providers_with_affinity(
        &providers,
        Some("shared-model"),
        RoutingPolicy::Priority,
        None,
        &pools,
        Some(&resilience),
        Some(PrefixAffinityRouting {
            index: &index,
            config: &config,
            fingerprint: &fingerprint,
        }),
    );

    assert_eq!(selection.reason, "circuit_open");
    assert_eq!(
        provider_names(&selection.candidates),
        vec!["replica-b", "replica-a"]
    );
}

#[test]
fn prefix_affinity_without_high_confidence_prefix_uses_fallback_policy() {
    let providers = vec![
        provider_entry("replica-a", "internal-replica-a", 1),
        provider_entry("replica-b", "internal-replica-b", 2),
    ];
    let pools = prefix_pool();
    let config = prefix_affinity_config();
    let index = PrefixAffinityIndex::new(&config);
    let fingerprint = low_confidence_fingerprint();

    let selection = candidate_providers_with_affinity(
        &providers,
        Some("shared-model"),
        RoutingPolicy::Priority,
        None,
        &pools,
        None,
        Some(PrefixAffinityRouting {
            index: &index,
            config: &config,
            fingerprint: &fingerprint,
        }),
    );

    assert_eq!(selection.reason, "fallback");
    assert_eq!(
        provider_names(&selection.candidates),
        vec!["replica-a", "replica-b"]
    );
}

#[test]
fn fallback_decision_tries_next_provider_for_retryable_errors() {
    assert_eq!(
        fallback_decision(&ProviderError::Timeout),
        RetryDecision::TryNextProvider
    );
    assert_eq!(
        fallback_decision(&ProviderError::RateLimited),
        RetryDecision::TryNextProvider
    );
    assert_eq!(
        fallback_decision(&ProviderError::ProviderUnavailable),
        RetryDecision::TryNextProvider
    );
}

#[test]
fn fallback_decision_stops_for_non_retryable_errors() {
    assert_eq!(
        fallback_decision(&ProviderError::AuthenticationFailed),
        RetryDecision::Stop
    );
    assert_eq!(
        fallback_decision(&ProviderError::ProviderBadResponse),
        RetryDecision::Stop
    );
}
