//! Provider selection strategy.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::{
    config::{PrefixAffinityConfig, RoutingPolicy},
    providers::provider::ProviderEntry,
    routing::{
        model_pools::ModelPoolIndex,
        prefix_affinity::PrefixAffinityIndex,
        prefix_fingerprint::PrefixFingerprintResult,
        resilience::{CircuitState, ResilienceRegistry},
    },
    telemetry::metrics::MetricsSnapshot,
};

#[cfg(feature = "runtime-cache-signals")]
use crate::routing::runtime_signals::{RuntimeSignalSource, RuntimeWorkerSignal};

const IN_FLIGHT_LATENCY_PENALTY_MS: f64 = 25.0;
const QUEUE_PRESSURE_PENALTY_MS: f64 = 10.0;
const RECENT_ERROR_PENALTY_MS: f64 = 250.0;
const HALF_OPEN_CIRCUIT_PENALTY_MS: f64 = 10_000.0;
const OPEN_CIRCUIT_PENALTY_MS: f64 = 1_000_000.0;

pub struct CandidateSelection<'a> {
    pub candidates: Vec<&'a ProviderEntry>,
    pub effective_policy: RoutingPolicy,
    pub reason: &'static str,
}

pub struct PrefixAffinityRouting<'a> {
    pub index: &'a PrefixAffinityIndex,
    pub config: &'a PrefixAffinityConfig,
    pub fingerprint: &'a PrefixFingerprintResult,
}

#[cfg(feature = "runtime-cache-signals")]
pub struct RuntimeSignalRouting<'a> {
    pub source: &'a dyn RuntimeSignalSource,
    pub prefix_fingerprint: Option<&'a str>,
}

#[cfg(feature = "runtime-cache-signals")]
pub struct RuntimeSignalSelection<'a> {
    pub candidates: Vec<&'a ProviderEntry>,
    pub reason: &'static str,
}

pub fn select_provider<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    model_pools: &ModelPoolIndex,
) -> Option<&'a ProviderEntry> {
    candidate_providers(
        providers,
        requested_model,
        RoutingPolicy::Priority,
        None,
        model_pools,
        None,
    )
    .into_iter()
    .next()
}

pub fn candidate_providers<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
    model_pools: &ModelPoolIndex,
    resilience: Option<&ResilienceRegistry>,
) -> Vec<&'a ProviderEntry> {
    let effective_policy = effective_routing_policy(requested_model, routing_policy, model_pools);
    candidate_providers_for_policy(
        providers,
        requested_model,
        effective_policy,
        metrics_snapshot,
        model_pools,
        resilience,
    )
}

pub fn candidate_providers_with_affinity<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
    model_pools: &ModelPoolIndex,
    resilience: Option<&ResilienceRegistry>,
    prefix_affinity: Option<PrefixAffinityRouting<'_>>,
) -> CandidateSelection<'a> {
    let Some(requested_model) = requested_model else {
        return CandidateSelection {
            candidates: Vec::new(),
            effective_policy: routing_policy,
            reason: "no_provider",
        };
    };

    let pool = model_pools.pool_for_public_model(requested_model);
    let effective_policy =
        effective_routing_policy(Some(requested_model), routing_policy, model_pools);
    if effective_policy != RoutingPolicy::PrefixAffinity {
        let candidates = candidate_providers_for_policy(
            providers,
            Some(requested_model),
            effective_policy,
            metrics_snapshot,
            model_pools,
            resilience,
        );
        let reason = routing_decision_reason(
            effective_policy,
            candidates.first().copied(),
            metrics_snapshot,
        );
        return CandidateSelection {
            candidates,
            effective_policy,
            reason,
        };
    }

    let Some(prefix_affinity) = prefix_affinity else {
        return prefix_affinity_fallback_selection(
            providers,
            Some(requested_model),
            effective_policy,
            RoutingPolicy::Priority,
            metrics_snapshot,
            model_pools,
            resilience,
            "fallback",
        );
    };

    let baseline_policy = prefix_affinity.config.fallback_policy;
    let Some(pool) = pool else {
        return prefix_affinity_fallback_selection(
            providers,
            Some(requested_model),
            effective_policy,
            baseline_policy,
            metrics_snapshot,
            model_pools,
            resilience,
            "fallback",
        );
    };
    if pool.members.len() <= 1 || !prefix_affinity.fingerprint.is_high_confidence() {
        return prefix_affinity_fallback_selection(
            providers,
            Some(requested_model),
            effective_policy,
            baseline_policy,
            metrics_snapshot,
            model_pools,
            resilience,
            "fallback",
        );
    }

    let Some(fingerprint) = prefix_affinity.fingerprint.fingerprint.as_deref() else {
        return prefix_affinity_fallback_selection(
            providers,
            Some(requested_model),
            effective_policy,
            baseline_policy,
            metrics_snapshot,
            model_pools,
            resilience,
            "fallback",
        );
    };

    let baseline = candidate_providers_for_policy(
        providers,
        Some(requested_model),
        baseline_policy,
        metrics_snapshot,
        model_pools,
        resilience,
    );
    if baseline.is_empty() {
        return CandidateSelection {
            candidates: baseline,
            effective_policy,
            reason: "no_provider",
        };
    }

    let healthy = baseline
        .iter()
        .copied()
        .filter(|entry| !is_open_circuit(entry, resilience))
        .collect::<Vec<_>>();
    if healthy.is_empty() {
        return CandidateSelection {
            candidates: order_open_circuits_last(baseline, resilience),
            effective_policy,
            reason: "circuit_open",
        };
    }

    if let Some(previous_provider) = prefix_affinity.index.lookup(fingerprint) {
        if baseline
            .iter()
            .any(|entry| entry.provider.name() == previous_provider)
        {
            if healthy
                .iter()
                .all(|entry| entry.provider.name() != previous_provider)
            {
                return CandidateSelection {
                    candidates: order_open_circuits_last(baseline, resilience),
                    effective_policy,
                    reason: "circuit_open",
                };
            }
            if provider_has_recent_errors(metrics_snapshot, &previous_provider) {
                return CandidateSelection {
                    candidates: baseline,
                    effective_policy,
                    reason: "fallback",
                };
            }
            if load_within_threshold(
                metrics_snapshot,
                &previous_provider,
                &healthy,
                prefix_affinity.config.load_imbalance_threshold,
            ) {
                return CandidateSelection {
                    candidates: move_provider_to_front(baseline, &previous_provider),
                    effective_policy,
                    reason: "prefix_hit",
                };
            }

            return CandidateSelection {
                candidates: order_by_load_then_baseline(baseline, metrics_snapshot, resilience),
                effective_policy,
                reason: "load_imbalanced",
            };
        }
    }

    if let Some(selected) = rendezvous_provider(fingerprint, &healthy) {
        return CandidateSelection {
            candidates: move_provider_to_front(baseline, selected.provider.name()),
            effective_policy,
            reason: "prefix_miss",
        };
    }

    CandidateSelection {
        candidates: baseline,
        effective_policy,
        reason: "fallback",
    }
}

#[cfg(feature = "runtime-cache-signals")]
pub fn order_candidates_by_runtime_signals<'a>(
    candidates: Vec<&'a ProviderEntry>,
    runtime_signals: RuntimeSignalRouting<'_>,
) -> RuntimeSignalSelection<'a> {
    if candidates.len() <= 1 {
        return RuntimeSignalSelection {
            candidates,
            reason: "fallback",
        };
    }

    let baseline_positions = candidates
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.provider.name().to_string(), index))
        .collect::<BTreeMap<_, _>>();
    let signal_by_provider = candidates
        .iter()
        .filter_map(|entry| {
            runtime_signals
                .source
                .signal_for_provider(entry.provider.name())
                .map(|signal| (entry.provider.name().to_string(), signal))
        })
        .collect::<BTreeMap<_, _>>();

    if signal_by_provider.is_empty() {
        return RuntimeSignalSelection {
            candidates,
            reason: "fallback",
        };
    }

    let mut ordered = candidates;
    ordered.sort_by(|left, right| {
        runtime_signal_score(
            signal_by_provider.get(left.provider.name()),
            runtime_signals.prefix_fingerprint,
        )
        .total_cmp(&runtime_signal_score(
            signal_by_provider.get(right.provider.name()),
            runtime_signals.prefix_fingerprint,
        ))
        .then_with(|| {
            baseline_positions[left.provider.name()].cmp(&baseline_positions[right.provider.name()])
        })
        .then_with(|| left.provider.name().cmp(right.provider.name()))
    });

    let reason = ordered
        .first()
        .and_then(|entry| signal_by_provider.get(entry.provider.name()))
        .map(|selected| {
            runtime_signal_reason(
                selected,
                runtime_signals.prefix_fingerprint,
                &signal_by_provider,
            )
        })
        .unwrap_or("fallback");

    RuntimeSignalSelection {
        candidates: ordered,
        reason,
    }
}

fn candidate_providers_for_policy<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
    model_pools: &ModelPoolIndex,
    resilience: Option<&ResilienceRegistry>,
) -> Vec<&'a ProviderEntry> {
    let Some(requested_model) = requested_model else {
        return Vec::new();
    };

    let pool = model_pools.pool_for_public_model(requested_model);
    let mut candidates = providers
        .iter()
        .filter(|entry| {
            if let Some(pool) = pool {
                pool.members.contains(entry.provider.name())
            } else {
                entry.provider.supports_model(requested_model)
            }
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        match routing_policy {
            RoutingPolicy::Priority => {}
            RoutingPolicy::Cost => {
                let cost_order = provider_cost(left).total_cmp(&provider_cost(right));
                if !cost_order.is_eq() {
                    return cost_order;
                }
            }
            RoutingPolicy::Latency => {
                let latency_order = provider_latency_score(left, metrics_snapshot, resilience)
                    .total_cmp(&provider_latency_score(right, metrics_snapshot, resilience));
                if !latency_order.is_eq() {
                    return latency_order;
                }
            }
            RoutingPolicy::PrefixAffinity => {}
        }

        left.priority
            .cmp(&right.priority)
            .then_with(|| left.provider.name().cmp(right.provider.name()))
    });
    candidates
}

pub fn resolve_model_alias(aliases: &BTreeMap<String, String>, requested_model: &str) -> String {
    aliases
        .get(requested_model)
        .cloned()
        .unwrap_or_else(|| requested_model.to_string())
}

pub fn effective_routing_policy(
    requested_model: Option<&str>,
    routing_policy: RoutingPolicy,
    model_pools: &ModelPoolIndex,
) -> RoutingPolicy {
    requested_model
        .and_then(|model| model_pools.pool_for_public_model(model))
        .and_then(|pool| pool.routing_policy)
        .unwrap_or(routing_policy)
}

pub fn routing_decision_reason(
    routing_policy: RoutingPolicy,
    selected: Option<&ProviderEntry>,
    metrics_snapshot: Option<&MetricsSnapshot>,
) -> &'static str {
    match routing_policy {
        RoutingPolicy::Priority => "priority",
        RoutingPolicy::Cost => "lowest_cost",
        RoutingPolicy::PrefixAffinity => "fallback",
        RoutingPolicy::Latency => {
            let Some(selected) = selected else {
                return "no_provider";
            };
            let provider_name = selected.provider.name();
            if metrics_snapshot
                .and_then(|snapshot| snapshot.p95_latency_ms_by_provider.get(provider_name))
                .is_some_and(|latency| *latency > 0.0)
            {
                "recent_latency_p95"
            } else if metrics_snapshot
                .and_then(|snapshot| snapshot.avg_latency_ms_by_provider.get(provider_name))
                .is_some_and(|latency| *latency > 0.0)
            {
                "average_latency"
            } else {
                "no_latency_data"
            }
        }
    }
}

fn provider_cost(entry: &ProviderEntry) -> f64 {
    entry.pricing.cost_per_1k_input_tokens + entry.pricing.cost_per_1k_output_tokens
}

fn provider_latency_ms(entry: &ProviderEntry, metrics_snapshot: Option<&MetricsSnapshot>) -> f64 {
    metrics_snapshot
        .and_then(|snapshot| {
            snapshot
                .p95_latency_ms_by_provider
                .get(entry.provider.name())
                .copied()
                .filter(|latency| *latency > 0.0)
                .or_else(|| {
                    snapshot
                        .avg_latency_ms_by_provider
                        .get(entry.provider.name())
                        .copied()
                        .filter(|latency| *latency > 0.0)
                })
        })
        .unwrap_or(f64::INFINITY)
}

fn provider_latency_score(
    entry: &ProviderEntry,
    metrics_snapshot: Option<&MetricsSnapshot>,
    resilience: Option<&ResilienceRegistry>,
) -> f64 {
    let provider_name = entry.provider.name();
    let mut score = provider_latency_ms(entry, metrics_snapshot);

    if let Some(snapshot) = metrics_snapshot {
        score += snapshot
            .in_flight_requests_by_provider
            .get(provider_name)
            .copied()
            .unwrap_or_default() as f64
            * IN_FLIGHT_LATENCY_PENALTY_MS;
        score += snapshot
            .queue_pressure_by_provider
            .get(provider_name)
            .copied()
            .unwrap_or_default()
            * QUEUE_PRESSURE_PENALTY_MS;
        score += recent_error_count(snapshot, provider_name) as f64 * RECENT_ERROR_PENALTY_MS;
    }

    if let Some(resilience) = resilience {
        score += match resilience.circuit_state(provider_name) {
            CircuitState::Closed => 0.0,
            CircuitState::HalfOpen => HALF_OPEN_CIRCUIT_PENALTY_MS,
            CircuitState::Open => OPEN_CIRCUIT_PENALTY_MS,
        };
    }

    score
}

fn recent_error_count(snapshot: &MetricsSnapshot, provider_name: &str) -> u64 {
    snapshot
        .recent_provider_errors_by_provider_and_category
        .get(provider_name)
        .map(|categories| categories.values().sum())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn prefix_affinity_fallback_selection<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    effective_policy: RoutingPolicy,
    fallback_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
    model_pools: &ModelPoolIndex,
    resilience: Option<&ResilienceRegistry>,
    reason: &'static str,
) -> CandidateSelection<'a> {
    CandidateSelection {
        candidates: candidate_providers_for_policy(
            providers,
            requested_model,
            fallback_policy,
            metrics_snapshot,
            model_pools,
            resilience,
        ),
        effective_policy,
        reason,
    }
}

fn is_open_circuit(entry: &ProviderEntry, resilience: Option<&ResilienceRegistry>) -> bool {
    resilience
        .is_some_and(|registry| registry.circuit_state(entry.provider.name()) == CircuitState::Open)
}

fn provider_has_recent_errors(
    metrics_snapshot: Option<&MetricsSnapshot>,
    provider_name: &str,
) -> bool {
    metrics_snapshot.is_some_and(|snapshot| recent_error_count(snapshot, provider_name) > 0)
}

fn load_within_threshold(
    metrics_snapshot: Option<&MetricsSnapshot>,
    provider_name: &str,
    candidates: &[&ProviderEntry],
    threshold: u64,
) -> bool {
    let Some(snapshot) = metrics_snapshot else {
        return true;
    };

    let current_in_flight = provider_in_flight(snapshot, provider_name);
    let current_queue_pressure = provider_queue_pressure(snapshot, provider_name);
    let min_in_flight = candidates
        .iter()
        .map(|entry| provider_in_flight(snapshot, entry.provider.name()))
        .min()
        .unwrap_or_default();
    let min_queue_pressure = candidates
        .iter()
        .map(|entry| provider_queue_pressure(snapshot, entry.provider.name()))
        .min()
        .unwrap_or_default();

    current_in_flight <= min_in_flight.saturating_add(threshold)
        && current_queue_pressure <= min_queue_pressure.saturating_add(threshold)
}

fn provider_in_flight(snapshot: &MetricsSnapshot, provider_name: &str) -> u64 {
    snapshot
        .in_flight_requests_by_provider
        .get(provider_name)
        .copied()
        .unwrap_or_default()
}

fn provider_queue_pressure(snapshot: &MetricsSnapshot, provider_name: &str) -> u64 {
    snapshot
        .queue_pressure_by_provider
        .get(provider_name)
        .copied()
        .unwrap_or_default()
        .ceil() as u64
}

fn move_provider_to_front<'a>(
    candidates: Vec<&'a ProviderEntry>,
    provider_name: &str,
) -> Vec<&'a ProviderEntry> {
    let mut preferred = Vec::with_capacity(candidates.len());
    let mut rest = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        if candidate.provider.name() == provider_name {
            preferred.push(candidate);
        } else {
            rest.push(candidate);
        }
    }

    preferred.extend(rest);
    preferred
}

fn order_open_circuits_last<'a>(
    mut candidates: Vec<&'a ProviderEntry>,
    resilience: Option<&ResilienceRegistry>,
) -> Vec<&'a ProviderEntry> {
    candidates.sort_by(|left, right| {
        is_open_circuit(left, resilience)
            .cmp(&is_open_circuit(right, resilience))
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.provider.name().cmp(right.provider.name()))
    });
    candidates
}

fn order_by_load_then_baseline<'a>(
    mut candidates: Vec<&'a ProviderEntry>,
    metrics_snapshot: Option<&MetricsSnapshot>,
    resilience: Option<&ResilienceRegistry>,
) -> Vec<&'a ProviderEntry> {
    let baseline_positions = candidates
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.provider.name().to_string(), index))
        .collect::<BTreeMap<_, _>>();

    candidates.sort_by(|left, right| {
        load_score(left, metrics_snapshot, resilience)
            .cmp(&load_score(right, metrics_snapshot, resilience))
            .then_with(|| {
                baseline_positions[left.provider.name()]
                    .cmp(&baseline_positions[right.provider.name()])
            })
            .then_with(|| left.provider.name().cmp(right.provider.name()))
    });
    candidates
}

fn load_score(
    entry: &ProviderEntry,
    metrics_snapshot: Option<&MetricsSnapshot>,
    resilience: Option<&ResilienceRegistry>,
) -> u64 {
    let provider_name = entry.provider.name();
    let circuit_penalty = if is_open_circuit(entry, resilience) {
        1_000_000
    } else {
        0
    };
    let Some(snapshot) = metrics_snapshot else {
        return circuit_penalty;
    };

    provider_in_flight(snapshot, provider_name)
        .max(provider_queue_pressure(snapshot, provider_name))
        .saturating_add(recent_error_count(snapshot, provider_name).saturating_mul(1_000))
        .saturating_add(circuit_penalty)
}

fn rendezvous_provider<'a>(
    fingerprint: &str,
    candidates: &[&'a ProviderEntry],
) -> Option<&'a ProviderEntry> {
    candidates.iter().copied().max_by(|left, right| {
        rendezvous_score(fingerprint, left.provider.name())
            .cmp(&rendezvous_score(fingerprint, right.provider.name()))
            .then_with(|| right.provider.name().cmp(left.provider.name()))
    })
}

fn rendezvous_score(fingerprint: &str, provider_name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(fingerprint.as_bytes());
    hasher.update(b":");
    hasher.update(provider_name.as_bytes());
    let digest = hasher.finalize();
    let mut score = [0_u8; 32];
    score.copy_from_slice(&digest);
    score
}

#[cfg(feature = "runtime-cache-signals")]
fn runtime_signal_score(
    signal: Option<&RuntimeWorkerSignal>,
    prefix_fingerprint: Option<&str>,
) -> f64 {
    let Some(signal) = signal else {
        return f64::INFINITY;
    };

    let prefix_residency_bonus = prefix_fingerprint
        .and_then(|fingerprint| signal.prefix_residency(fingerprint))
        .map(|residency| residency.resident_fraction() * 10_000.0)
        .unwrap_or_default();
    let cache_hit_bonus = signal.cache_hit_fraction.clamp(0.0, 1.0) * 1_000.0;
    let queue_penalty = signal.queue_depth as f64 * 200.0;
    let in_flight_penalty = signal.in_flight as f64 * 50.0;
    let kv_utilization_penalty = signal.kv_cache_utilization.clamp(0.0, 1.0) * 1_000.0;

    queue_penalty + in_flight_penalty + kv_utilization_penalty
        - prefix_residency_bonus
        - cache_hit_bonus
}

#[cfg(feature = "runtime-cache-signals")]
fn runtime_signal_reason(
    selected: &RuntimeWorkerSignal,
    prefix_fingerprint: Option<&str>,
    signals: &BTreeMap<String, RuntimeWorkerSignal>,
) -> &'static str {
    if prefix_fingerprint.is_some_and(|fingerprint| {
        selected
            .prefix_residency(fingerprint)
            .is_some_and(|residency| residency.resident_fraction() > 0.0)
    }) {
        return "runtime_prefix_resident";
    }

    if has_better_queue_depth(selected, signals) {
        return "runtime_queue_depth";
    }

    if has_better_kv_headroom(selected, signals) {
        return "runtime_kv_headroom";
    }

    if selected.cache_hit_fraction > 0.0 {
        return "runtime_cache_hit";
    }

    "fallback"
}

#[cfg(feature = "runtime-cache-signals")]
fn has_better_queue_depth(
    selected: &RuntimeWorkerSignal,
    signals: &BTreeMap<String, RuntimeWorkerSignal>,
) -> bool {
    let selected_load = selected.queue_depth.saturating_add(selected.in_flight);
    signals
        .values()
        .any(|signal| signal.queue_depth.saturating_add(signal.in_flight) > selected_load)
}

#[cfg(feature = "runtime-cache-signals")]
fn has_better_kv_headroom(
    selected: &RuntimeWorkerSignal,
    signals: &BTreeMap<String, RuntimeWorkerSignal>,
) -> bool {
    signals
        .values()
        .any(|signal| signal.kv_cache_utilization > selected.kv_cache_utilization)
}
