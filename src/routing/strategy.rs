//! Provider selection strategy scaffolding.
//!
//! MVP routing should start with exact model matching and then sort matching providers by priority.

use crate::providers::provider::ProviderEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    Priority,
}

pub fn select_provider<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
) -> Option<&'a ProviderEntry> {
    candidate_providers(providers, requested_model)
        .into_iter()
        .next()
}

pub fn candidate_providers<'a>(
    providers: &'a [ProviderEntry],
    requested_model: Option<&str>,
) -> Vec<&'a ProviderEntry> {
    let Some(requested_model) = requested_model else {
        return Vec::new();
    };

    let mut candidates = providers
        .iter()
        .filter(|entry| entry.provider.supports_model(requested_model))
        .collect::<Vec<_>>();

    candidates.sort_by_key(|entry| entry.priority);
    candidates
}
