use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::time::sleep;
use tracing::warn;

use crate::{
    app::AppState,
    providers::provider::ProviderError,
    routing::fallback::{provider_error_category, ProviderErrorCategory},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealthStatus {
    Unknown,
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct ProviderHealthSnapshot {
    pub status: ProviderHealthStatus,
    pub checked_at_unix_seconds: Option<u64>,
    pub error_category: Option<ProviderErrorCategory>,
}

impl Default for ProviderHealthSnapshot {
    fn default() -> Self {
        Self {
            status: ProviderHealthStatus::Unknown,
            checked_at_unix_seconds: None,
            error_category: None,
        }
    }
}

#[derive(Debug)]
pub struct ProviderHealthRegistry {
    providers: RwLock<BTreeMap<String, ProviderHealthSnapshot>>,
}

impl ProviderHealthRegistry {
    pub fn new(provider_names: &[String]) -> Self {
        let providers = provider_names
            .iter()
            .map(|name| (name.clone(), ProviderHealthSnapshot::default()))
            .collect();
        Self {
            providers: RwLock::new(providers),
        }
    }

    pub fn record_success(&self, provider: &str) {
        self.record(provider, ProviderHealthStatus::Healthy, None);
    }

    pub fn record_failure(&self, provider: &str, error: &ProviderError) {
        self.record(
            provider,
            ProviderHealthStatus::Unhealthy,
            Some(provider_error_category(error)),
        );
    }

    pub fn snapshot(&self) -> BTreeMap<String, ProviderHealthSnapshot> {
        self.providers
            .read()
            .map(|providers| providers.clone())
            .unwrap_or_default()
    }

    pub fn any_provider_ready(&self) -> bool {
        let snapshot = self.snapshot();
        !snapshot.is_empty()
            && snapshot
                .values()
                .any(|snapshot| snapshot.status != ProviderHealthStatus::Unhealthy)
    }

    fn record(
        &self,
        provider: &str,
        status: ProviderHealthStatus,
        error_category: Option<ProviderErrorCategory>,
    ) {
        if let Ok(mut providers) = self.providers.write() {
            providers.insert(
                provider.to_string(),
                ProviderHealthSnapshot {
                    status,
                    checked_at_unix_seconds: Some(unix_seconds()),
                    error_category,
                },
            );
        }
    }
}

pub fn spawn_provider_health_probes(state: AppState, interval: Duration) {
    tokio::spawn(async move {
        loop {
            probe_once(&state).await;
            sleep(interval).await;
        }
    });
}

pub async fn probe_once(state: &AppState) {
    for entry in &state.providers {
        let provider_name = entry.provider.name().to_string();
        let result = entry.provider.health_check().await;
        match result {
            Ok(()) => state.provider_health.record_success(&provider_name),
            Err(error) => {
                warn!(
                    provider = provider_name,
                    error_category = provider_error_category(&error).as_str(),
                    "provider health check failed"
                );
                state.provider_health.record_failure(&provider_name, &error);
            }
        }
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub type SharedProviderHealthRegistry = Arc<ProviderHealthRegistry>;
