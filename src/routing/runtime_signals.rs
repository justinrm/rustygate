//! Experimental runtime cache/load signal types.
//!
//! These types describe signals a self-hosted inference worker could expose. They are feature
//! gated because normal RustyGate routing should not depend on runtime-specific telemetry.

use std::collections::BTreeMap;

/// Identifies the runtime worker backing a configured provider or pool member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimeWorkerId {
    pub provider_name: String,
    pub runtime_id: String,
}

impl RuntimeWorkerId {
    pub fn new(provider_name: impl Into<String>, runtime_id: impl Into<String>) -> Self {
        Self {
            provider_name: provider_name.into(),
            runtime_id: runtime_id.into(),
        }
    }
}

/// Runtime-side evidence that a hashed prefix may be resident on a worker.
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixResidency {
    pub fingerprint: String,
    pub resident_blocks: u64,
    pub total_blocks: u64,
}

impl PrefixResidency {
    pub fn new(fingerprint: impl Into<String>, resident_blocks: u64, total_blocks: u64) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            resident_blocks,
            total_blocks,
        }
    }

    pub fn resident_fraction(&self) -> f64 {
        if self.total_blocks == 0 {
            return 0.0;
        }

        (self.resident_blocks as f64 / self.total_blocks as f64).clamp(0.0, 1.0)
    }
}

/// Snapshot of runtime-observed cache and queue state for one worker.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeWorkerSignal {
    pub worker: RuntimeWorkerId,
    pub queue_depth: u64,
    pub in_flight: u64,
    pub kv_cache_utilization: f64,
    pub cache_hit_fraction: f64,
    prefix_residency_by_fingerprint: BTreeMap<String, PrefixResidency>,
}

impl RuntimeWorkerSignal {
    pub fn new(provider_name: impl Into<String>) -> Self {
        let provider_name = provider_name.into();
        Self {
            worker: RuntimeWorkerId::new(provider_name.clone(), provider_name),
            queue_depth: 0,
            in_flight: 0,
            kv_cache_utilization: 0.0,
            cache_hit_fraction: 0.0,
            prefix_residency_by_fingerprint: BTreeMap::new(),
        }
    }

    pub fn with_runtime_id(mut self, runtime_id: impl Into<String>) -> Self {
        self.worker.runtime_id = runtime_id.into();
        self
    }

    pub fn with_queue_depth(mut self, queue_depth: u64) -> Self {
        self.queue_depth = queue_depth;
        self
    }

    pub fn with_in_flight(mut self, in_flight: u64) -> Self {
        self.in_flight = in_flight;
        self
    }

    pub fn with_kv_cache_utilization(mut self, kv_cache_utilization: f64) -> Self {
        self.kv_cache_utilization = kv_cache_utilization.clamp(0.0, 1.0);
        self
    }

    pub fn with_cache_hit_fraction(mut self, cache_hit_fraction: f64) -> Self {
        self.cache_hit_fraction = cache_hit_fraction.clamp(0.0, 1.0);
        self
    }

    pub fn with_prefix_residency(mut self, residency: PrefixResidency) -> Self {
        self.prefix_residency_by_fingerprint
            .insert(residency.fingerprint.clone(), residency);
        self
    }

    pub fn prefix_residency(&self, fingerprint: &str) -> Option<&PrefixResidency> {
        self.prefix_residency_by_fingerprint.get(fingerprint)
    }
}

/// Read-only source of runtime cache/load signals.
pub trait RuntimeSignalSource: Send + Sync {
    fn signal_for_provider(&self, provider_name: &str) -> Option<RuntimeWorkerSignal>;
}

/// Deterministic in-memory signal source for tests and local design experiments.
#[derive(Debug, Clone, Default)]
pub struct MockRuntimeSignalSource {
    signals_by_provider: BTreeMap<String, RuntimeWorkerSignal>,
}

impl MockRuntimeSignalSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_signal(mut self, signal: RuntimeWorkerSignal) -> Self {
        self.insert_signal(signal);
        self
    }

    pub fn insert_signal(&mut self, signal: RuntimeWorkerSignal) {
        self.signals_by_provider
            .insert(signal.worker.provider_name.clone(), signal);
    }
}

impl RuntimeSignalSource for MockRuntimeSignalSource {
    fn signal_for_provider(&self, provider_name: &str) -> Option<RuntimeWorkerSignal> {
        self.signals_by_provider.get(provider_name).cloned()
    }
}
