#![cfg(feature = "runtime-cache-signals")]

use rustygate::{
    providers::provider::ProviderEntry,
    routing::{
        runtime_signals::{MockRuntimeSignalSource, PrefixResidency, RuntimeWorkerSignal},
        strategy::{order_candidates_by_runtime_signals, RuntimeSignalRouting},
    },
};

mod common;

use common::mock_provider_entry;

fn provider_entry(name: &str, model: &str, priority: u32) -> ProviderEntry {
    mock_provider_entry(name, model, priority)
}

fn order_with_signals(
    providers: &[ProviderEntry],
    signals: &MockRuntimeSignalSource,
    prefix_fingerprint: Option<&str>,
) -> (Vec<String>, &'static str) {
    let selection = order_candidates_by_runtime_signals(
        providers.iter().collect::<Vec<_>>(),
        RuntimeSignalRouting {
            source: signals,
            prefix_fingerprint,
        },
    );
    let names = selection
        .candidates
        .iter()
        .map(|entry| entry.provider.name().to_string())
        .collect::<Vec<_>>();

    (names, selection.reason)
}

#[test]
fn runtime_signals_prefer_explicit_prefix_residency() {
    let providers = vec![
        provider_entry("replica-a", "internal-a", 1),
        provider_entry("replica-b", "internal-b", 2),
    ];
    let signals = MockRuntimeSignalSource::new()
        .with_signal(RuntimeWorkerSignal::new("replica-a").with_cache_hit_fraction(0.9))
        .with_signal(
            RuntimeWorkerSignal::new("replica-b")
                .with_cache_hit_fraction(0.1)
                .with_prefix_residency(PrefixResidency::new("prefix-a", 4, 4)),
        );

    let (names, reason) = order_with_signals(&providers, &signals, Some("prefix-a"));

    assert_eq!(names, vec!["replica-b", "replica-a"]);
    assert_eq!(reason, "runtime_prefix_resident");
}

#[test]
fn runtime_signals_prefer_higher_cache_hit_fraction_under_equal_load() {
    let providers = vec![
        provider_entry("replica-a", "internal-a", 1),
        provider_entry("replica-b", "internal-b", 2),
    ];
    let signals = MockRuntimeSignalSource::new()
        .with_signal(RuntimeWorkerSignal::new("replica-a").with_cache_hit_fraction(0.1))
        .with_signal(RuntimeWorkerSignal::new("replica-b").with_cache_hit_fraction(0.8));

    let (names, reason) = order_with_signals(&providers, &signals, None);

    assert_eq!(names, vec!["replica-b", "replica-a"]);
    assert_eq!(reason, "runtime_cache_hit");
}

#[test]
fn runtime_signals_prefer_lower_queue_depth_when_cache_is_tied() {
    let providers = vec![
        provider_entry("replica-a", "internal-a", 1),
        provider_entry("replica-b", "internal-b", 2),
    ];
    let signals = MockRuntimeSignalSource::new()
        .with_signal(
            RuntimeWorkerSignal::new("replica-a")
                .with_cache_hit_fraction(0.5)
                .with_queue_depth(5),
        )
        .with_signal(RuntimeWorkerSignal::new("replica-b").with_cache_hit_fraction(0.5));

    let (names, reason) = order_with_signals(&providers, &signals, None);

    assert_eq!(names, vec!["replica-b", "replica-a"]);
    assert_eq!(reason, "runtime_queue_depth");
}

#[test]
fn runtime_signals_avoid_high_kv_utilization() {
    let providers = vec![
        provider_entry("replica-a", "internal-a", 1),
        provider_entry("replica-b", "internal-b", 2),
    ];
    let signals = MockRuntimeSignalSource::new()
        .with_signal(
            RuntimeWorkerSignal::new("replica-a")
                .with_cache_hit_fraction(1.0)
                .with_kv_cache_utilization(0.95),
        )
        .with_signal(
            RuntimeWorkerSignal::new("replica-b")
                .with_cache_hit_fraction(0.5)
                .with_kv_cache_utilization(0.1),
        );

    let (names, reason) = order_with_signals(&providers, &signals, None);

    assert_eq!(names, vec!["replica-b", "replica-a"]);
    assert_eq!(reason, "runtime_kv_headroom");
}

#[test]
fn runtime_signals_fall_back_to_existing_order_when_missing() {
    let providers = vec![
        provider_entry("replica-a", "internal-a", 1),
        provider_entry("replica-b", "internal-b", 2),
    ];
    let signals = MockRuntimeSignalSource::new();

    let (names, reason) = order_with_signals(&providers, &signals, Some("prefix-a"));

    assert_eq!(names, vec!["replica-a", "replica-b"]);
    assert_eq!(reason, "fallback");
}
