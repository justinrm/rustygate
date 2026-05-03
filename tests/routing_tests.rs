use std::sync::Arc;

use rustygate::{
    providers::{
        mock::MockProvider,
        provider::{ChatProvider, ProviderEntry, ProviderError, ProviderPricing},
    },
    routing::{
        fallback::{fallback_decision, RetryDecision},
        strategy::{candidate_providers, select_provider},
    },
};

fn provider_entry(name: &str, model: &str, priority: u32) -> ProviderEntry {
    ProviderEntry {
        priority,
        provider: Arc::new(MockProvider::new(name, model)) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}

#[test]
fn select_provider_matches_requested_model() {
    let providers = vec![
        provider_entry("mock-fast", "mock-fast-v1", 3),
        provider_entry("mock-smart", "mock-smart-v1", 1),
    ];

    let selected = select_provider(&providers, Some("mock-fast-v1")).expect("provider exists");

    assert_eq!(selected.provider.name(), "mock-fast");
}

#[test]
fn select_provider_prefers_lowest_priority_when_multiple_support_requested_model() {
    let providers = vec![
        provider_entry("mock-secondary", "mock-fast-v1", 5),
        provider_entry("mock-primary", "mock-fast-v1", 1),
        provider_entry("mock-tertiary", "mock-fast-v1", 3),
    ];

    let selected = select_provider(&providers, Some("mock-fast-v1")).expect("provider exists");

    assert_eq!(selected.provider.name(), "mock-primary");
}

#[test]
fn select_provider_without_requested_model_returns_none() {
    let providers = vec![
        provider_entry("mock-third", "mock-third-v1", 3),
        provider_entry("mock-first", "mock-first-v1", 1),
        provider_entry("mock-second", "mock-second-v1", 2),
    ];

    let selected = select_provider(&providers, None);

    assert!(selected.is_none());
}

#[test]
fn select_provider_returns_none_for_unsupported_requested_model() {
    let providers = vec![
        provider_entry("mock-fast", "mock-fast-v1", 1),
        provider_entry("mock-smart", "mock-smart-v1", 2),
    ];

    let selected = select_provider(&providers, Some("unknown-model"));

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

    let selected = candidate_providers(&providers, Some("mock-fast-v1"));
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
