//! Provider selection strategy.

use std::collections::BTreeMap;

use crate::{
    config::RoutingPolicy, providers::provider::ProviderEntry, telemetry::metrics::MetricsSnapshot,
};

pub fn select_provider<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
) -> Option<&'a ProviderEntry> {
    candidate_providers(providers, requested_model, RoutingPolicy::Priority, None)
        .into_iter()
        .next()
}

pub fn candidate_providers<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
) -> Vec<&'a ProviderEntry> {
    let Some(requested_model) = requested_model else {
        return Vec::new();
    };

    let mut candidates = providers
        .iter()
        .filter(|entry| entry.provider.supports_model(requested_model))
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
                let latency_order = provider_latency_ms(left, metrics_snapshot)
                    .total_cmp(&provider_latency_ms(right, metrics_snapshot));
                if !latency_order.is_eq() {
                    return latency_order;
                }
            }
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

fn provider_cost(entry: &ProviderEntry) -> f64 {
    entry.pricing.cost_per_1k_input_tokens + entry.pricing.cost_per_1k_output_tokens
}

fn provider_latency_ms(entry: &ProviderEntry, metrics_snapshot: Option<&MetricsSnapshot>) -> f64 {
    metrics_snapshot
        .and_then(|snapshot| {
            snapshot
                .avg_latency_ms_by_provider
                .get(entry.provider.name())
                .copied()
        })
        .unwrap_or(f64::INFINITY)
}
