use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub jitter_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            initial_backoff_ms: 100,
            max_backoff_ms: 2_000,
            jitter_ms: 50,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerPolicy {
    pub failure_threshold: u32,
    pub open_duration_ms: u64,
    pub half_open_max_probes: u32,
}

impl Default for CircuitBreakerPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            open_duration_ms: 5_000,
            half_open_max_probes: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderResiliencePolicy {
    pub timeout_ms: Option<u64>,
    pub retry: RetryPolicy,
    pub breaker: CircuitBreakerPolicy,
}

#[derive(Debug, Clone, Copy)]
struct ProviderCircuitState {
    state: CircuitState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
    half_open_probes_used: u32,
}

impl Default for ProviderCircuitState {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            opened_at: None,
            half_open_probes_used: 0,
        }
    }
}

#[derive(Debug)]
pub struct ResilienceRegistry {
    default_policy: ProviderResiliencePolicy,
    provider_policies: HashMap<String, ProviderResiliencePolicy>,
    circuits: Mutex<HashMap<String, ProviderCircuitState>>,
}

impl ResilienceRegistry {
    pub fn new(
        default_policy: ProviderResiliencePolicy,
        provider_policies: HashMap<String, ProviderResiliencePolicy>,
        provider_names: &[String],
    ) -> Self {
        let mut circuits = HashMap::with_capacity(provider_names.len());
        for provider in provider_names {
            circuits.insert(provider.clone(), ProviderCircuitState::default());
        }
        Self {
            default_policy,
            provider_policies,
            circuits: Mutex::new(circuits),
        }
    }

    pub fn policy_for(&self, provider_name: &str) -> ProviderResiliencePolicy {
        self.provider_policies
            .get(provider_name)
            .copied()
            .unwrap_or(self.default_policy)
    }

    pub fn allow_provider_call(&self, provider_name: &str) -> bool {
        let now = Instant::now();
        let policy = self.policy_for(provider_name);
        let mut circuits = match self.circuits.lock() {
            Ok(circuits) => circuits,
            Err(_) => return true,
        };
        let state = circuits
            .entry(provider_name.to_string())
            .or_insert_with(ProviderCircuitState::default);

        match state.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                let ready_for_probe = state.opened_at.is_none_or(|opened| {
                    now.duration_since(opened)
                        >= Duration::from_millis(policy.breaker.open_duration_ms)
                });
                if ready_for_probe {
                    state.state = CircuitState::HalfOpen;
                    state.half_open_probes_used = 1;
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                let max_probes = policy.breaker.half_open_max_probes.max(1);
                if state.half_open_probes_used < max_probes {
                    state.half_open_probes_used += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn record_success(&self, provider_name: &str) {
        let mut circuits = match self.circuits.lock() {
            Ok(circuits) => circuits,
            Err(_) => return,
        };
        let state = circuits
            .entry(provider_name.to_string())
            .or_insert_with(ProviderCircuitState::default);
        state.state = CircuitState::Closed;
        state.consecutive_failures = 0;
        state.opened_at = None;
        state.half_open_probes_used = 0;
    }

    pub fn record_failure(&self, provider_name: &str) {
        let now = Instant::now();
        let policy = self.policy_for(provider_name);
        let threshold = policy.breaker.failure_threshold.max(1);
        let mut circuits = match self.circuits.lock() {
            Ok(circuits) => circuits,
            Err(_) => return,
        };
        let state = circuits
            .entry(provider_name.to_string())
            .or_insert_with(ProviderCircuitState::default);

        match state.state {
            CircuitState::Closed => {
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                if state.consecutive_failures >= threshold {
                    state.state = CircuitState::Open;
                    state.opened_at = Some(now);
                    state.half_open_probes_used = 0;
                }
            }
            CircuitState::HalfOpen => {
                state.state = CircuitState::Open;
                state.opened_at = Some(now);
                state.half_open_probes_used = 0;
            }
            CircuitState::Open => {}
        }
    }

    pub fn circuit_state(&self, provider_name: &str) -> CircuitState {
        self.circuits
            .lock()
            .ok()
            .and_then(|circuits| circuits.get(provider_name).copied())
            .unwrap_or_default()
            .state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_provider_registry(
        policy: ProviderResiliencePolicy,
        name: &str,
    ) -> ResilienceRegistry {
        let mut policies = HashMap::new();
        policies.insert(name.to_string(), policy);
        ResilienceRegistry::new(
            ProviderResiliencePolicy::default(),
            policies,
            &[name.to_string()],
        )
    }

    #[test]
    fn breaker_transitions_to_open_after_threshold_failures() {
        let registry = single_provider_registry(
            ProviderResiliencePolicy {
                breaker: CircuitBreakerPolicy {
                    failure_threshold: 2,
                    open_duration_ms: 10_000,
                    half_open_max_probes: 1,
                },
                ..ProviderResiliencePolicy::default()
            },
            "mock-primary",
        );

        assert!(registry.allow_provider_call("mock-primary"));
        registry.record_failure("mock-primary");
        assert_eq!(registry.circuit_state("mock-primary"), CircuitState::Closed);

        registry.record_failure("mock-primary");
        assert_eq!(registry.circuit_state("mock-primary"), CircuitState::Open);
        assert!(!registry.allow_provider_call("mock-primary"));
    }

    #[test]
    fn half_open_probe_success_closes_circuit() {
        let registry = single_provider_registry(
            ProviderResiliencePolicy {
                breaker: CircuitBreakerPolicy {
                    failure_threshold: 1,
                    open_duration_ms: 0,
                    half_open_max_probes: 1,
                },
                ..ProviderResiliencePolicy::default()
            },
            "mock-primary",
        );

        registry.record_failure("mock-primary");
        assert_eq!(registry.circuit_state("mock-primary"), CircuitState::Open);

        assert!(registry.allow_provider_call("mock-primary"));
        assert_eq!(
            registry.circuit_state("mock-primary"),
            CircuitState::HalfOpen
        );
        assert!(!registry.allow_provider_call("mock-primary"));

        registry.record_success("mock-primary");
        assert_eq!(registry.circuit_state("mock-primary"), CircuitState::Closed);
        assert!(registry.allow_provider_call("mock-primary"));
    }
}
