use std::{
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use lru::LruCache;

use crate::config::PrefixAffinityConfig;

#[derive(Debug)]
pub struct PrefixAffinityIndex {
    ttl: Duration,
    entries: Mutex<LruCache<String, PrefixAffinityEntry>>,
}

#[derive(Debug, Clone)]
struct PrefixAffinityEntry {
    provider_name: String,
    updated_at: Instant,
}

impl PrefixAffinityIndex {
    pub fn new(config: &PrefixAffinityConfig) -> Self {
        Self {
            ttl: Duration::from_secs(config.ttl_seconds),
            entries: Mutex::new(LruCache::new(
                NonZeroUsize::new(config.max_entries.max(1)).expect("max_entries is at least 1"),
            )),
        }
    }

    pub fn lookup(&self, fingerprint: &str) -> Option<String> {
        let now = Instant::now();
        let mut entries = self.entries.lock().ok()?;
        let expired = entries
            .get(fingerprint)
            .is_some_and(|entry| now.duration_since(entry.updated_at) > self.ttl);
        if expired {
            entries.pop(fingerprint);
            return None;
        }

        entries
            .get(fingerprint)
            .map(|entry| entry.provider_name.clone())
    }

    pub fn record(&self, fingerprint: &str, provider_name: &str) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };

        entries.put(
            fingerprint.to_string(),
            PrefixAffinityEntry {
                provider_name: provider_name.to_string(),
                updated_at: Instant::now(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingPolicy;

    fn config(max_entries: usize) -> PrefixAffinityConfig {
        PrefixAffinityConfig {
            ttl_seconds: 60,
            max_entries,
            load_imbalance_threshold: 2,
            fallback_policy: RoutingPolicy::Latency,
        }
    }

    #[test]
    fn records_and_looks_up_provider_for_fingerprint() {
        let index = PrefixAffinityIndex::new(&config(10));

        index.record("prefix-a", "replica-a");

        assert_eq!(index.lookup("prefix-a").as_deref(), Some("replica-a"));
        assert_eq!(index.lookup("prefix-b"), None);
    }

    #[test]
    fn evicts_least_recent_entry_when_capacity_is_reached() {
        let index = PrefixAffinityIndex::new(&config(1));

        index.record("prefix-a", "replica-a");
        index.record("prefix-b", "replica-b");

        assert_eq!(index.lookup("prefix-a"), None);
        assert_eq!(index.lookup("prefix-b").as_deref(), Some("replica-b"));
    }
}
