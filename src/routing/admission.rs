use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use uuid::Uuid;

use crate::{
    config::{AdmissionConfig, ModelPoolConfig, ProviderConfig},
    error::AppError,
    models::chat::ChatCompletionRequest,
    telemetry::token_estimator::estimate_tokens_for_messages,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdmissionRejectionReason {
    GlobalInFlightLimit,
    PoolInFlightLimit,
    ProviderInFlightLimit,
    MaxEstimatedPromptTokens,
    MaxEstimatedTotalTokens,
}

impl AdmissionRejectionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GlobalInFlightLimit => "global_in_flight_limit",
            Self::PoolInFlightLimit => "pool_in_flight_limit",
            Self::ProviderInFlightLimit => "provider_in_flight_limit",
            Self::MaxEstimatedPromptTokens => "max_estimated_prompt_tokens",
            Self::MaxEstimatedTotalTokens => "max_estimated_total_tokens",
        }
    }

    fn public_message(self) -> &'static str {
        match self {
            Self::GlobalInFlightLimit => "gateway global in-flight limit exceeded",
            Self::PoolInFlightLimit => "model pool in-flight limit exceeded",
            Self::ProviderInFlightLimit => "provider in-flight limit exceeded",
            Self::MaxEstimatedPromptTokens => "estimated prompt token limit exceeded",
            Self::MaxEstimatedTotalTokens => "estimated total token limit exceeded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdmissionRejection {
    pub reason: AdmissionRejectionReason,
    pub retry_after_seconds: u64,
}

impl AdmissionRejection {
    pub fn into_app_error(self, request_id: Option<Uuid>) -> AppError {
        AppError::AdmissionRejected {
            message: self.reason.public_message().into(),
            request_id,
            retry_after_seconds: self.retry_after_seconds,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AdmissionLimits {
    pub max_global_in_flight: Option<u64>,
    pub max_estimated_prompt_tokens: Option<u32>,
    pub max_estimated_total_tokens: Option<u32>,
    pub retry_after_seconds: u64,
    pool_max_in_flight: BTreeMap<String, u64>,
    provider_max_in_flight: BTreeMap<String, u64>,
}

impl AdmissionLimits {
    pub fn from_config(
        admission: &AdmissionConfig,
        providers: &[ProviderConfig],
        model_pools: &[ModelPoolConfig],
    ) -> Self {
        Self {
            max_global_in_flight: admission.max_global_in_flight,
            max_estimated_prompt_tokens: admission.max_estimated_prompt_tokens,
            max_estimated_total_tokens: admission.max_estimated_total_tokens,
            retry_after_seconds: admission.retry_after_seconds,
            pool_max_in_flight: model_pools
                .iter()
                .filter_map(|pool| pool.max_in_flight.map(|limit| (pool.name.clone(), limit)))
                .collect(),
            provider_max_in_flight: providers
                .iter()
                .filter_map(|provider| {
                    provider
                        .max_in_flight
                        .map(|limit| (provider.name.clone(), limit))
                })
                .collect(),
        }
    }

    fn retry_after_seconds(&self) -> u64 {
        self.retry_after_seconds.max(1)
    }
}

#[derive(Debug, Default)]
struct AdmissionCounters {
    global_in_flight: u64,
    pool_in_flight: BTreeMap<String, u64>,
    provider_in_flight: BTreeMap<String, u64>,
}

#[derive(Debug)]
pub struct AdmissionController {
    limits: AdmissionLimits,
    counters: Mutex<AdmissionCounters>,
}

impl AdmissionController {
    pub fn new(limits: AdmissionLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            counters: Mutex::new(AdmissionCounters::default()),
        })
    }

    pub fn disabled() -> Arc<Self> {
        Self::new(AdmissionLimits {
            retry_after_seconds: 1,
            ..AdmissionLimits::default()
        })
    }

    pub fn check_token_budget(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<(), AdmissionRejectionReason> {
        let prompt_tokens = estimate_tokens_for_messages(&request.messages);
        if self
            .limits
            .max_estimated_prompt_tokens
            .is_some_and(|limit| prompt_tokens > limit)
        {
            return Err(AdmissionRejectionReason::MaxEstimatedPromptTokens);
        }

        let total_tokens = prompt_tokens.saturating_add(request.max_tokens.unwrap_or_default());
        if self
            .limits
            .max_estimated_total_tokens
            .is_some_and(|limit| total_tokens > limit)
        {
            return Err(AdmissionRejectionReason::MaxEstimatedTotalTokens);
        }

        Ok(())
    }

    pub fn try_acquire_request(
        self: &Arc<Self>,
        pool_name: Option<&str>,
    ) -> Result<AdmissionGuard, AdmissionRejection> {
        let mut counters = self
            .counters
            .lock()
            .map_err(|_| self.rejection(AdmissionRejectionReason::GlobalInFlightLimit))?;
        let mut slots = Vec::new();

        if self
            .limits
            .max_global_in_flight
            .is_some_and(|limit| counters.global_in_flight >= limit)
        {
            return Err(self.rejection(AdmissionRejectionReason::GlobalInFlightLimit));
        }

        if let Some(pool_name) = pool_name {
            if let Some(limit) = self.limits.pool_max_in_flight.get(pool_name) {
                let current = counters
                    .pool_in_flight
                    .get(pool_name)
                    .copied()
                    .unwrap_or_default();
                if current >= *limit {
                    return Err(self.rejection(AdmissionRejectionReason::PoolInFlightLimit));
                }
            }
        }

        if self.limits.max_global_in_flight.is_some() {
            counters.global_in_flight = counters.global_in_flight.saturating_add(1);
            slots.push(AdmissionSlot::Global);
        }
        if let Some(pool_name) = pool_name {
            if self.limits.pool_max_in_flight.contains_key(pool_name) {
                *counters
                    .pool_in_flight
                    .entry(pool_name.to_string())
                    .or_default() += 1;
                slots.push(AdmissionSlot::Pool(pool_name.to_string()));
            }
        }

        Ok(AdmissionGuard::new(self.clone(), slots))
    }

    pub fn try_acquire_provider(
        self: &Arc<Self>,
        provider_name: &str,
    ) -> Result<AdmissionGuard, AdmissionRejection> {
        let Some(limit) = self
            .limits
            .provider_max_in_flight
            .get(provider_name)
            .copied()
        else {
            return Ok(AdmissionGuard::new(self.clone(), Vec::new()));
        };

        let mut counters = self
            .counters
            .lock()
            .map_err(|_| self.rejection(AdmissionRejectionReason::ProviderInFlightLimit))?;
        let current = counters
            .provider_in_flight
            .get(provider_name)
            .copied()
            .unwrap_or_default();
        if current >= limit {
            return Err(self.rejection(AdmissionRejectionReason::ProviderInFlightLimit));
        }

        *counters
            .provider_in_flight
            .entry(provider_name.to_string())
            .or_default() += 1;

        Ok(AdmissionGuard::new(
            self.clone(),
            vec![AdmissionSlot::Provider(provider_name.to_string())],
        ))
    }

    fn release(&self, slots: &[AdmissionSlot]) {
        let Ok(mut counters) = self.counters.lock() else {
            return;
        };
        for slot in slots {
            match slot {
                AdmissionSlot::Global => {
                    counters.global_in_flight = counters.global_in_flight.saturating_sub(1);
                }
                AdmissionSlot::Pool(pool_name) => {
                    decrement_or_remove(&mut counters.pool_in_flight, pool_name);
                }
                AdmissionSlot::Provider(provider_name) => {
                    decrement_or_remove(&mut counters.provider_in_flight, provider_name);
                }
            }
        }
    }

    fn rejection(&self, reason: AdmissionRejectionReason) -> AdmissionRejection {
        AdmissionRejection {
            reason,
            retry_after_seconds: self.limits.retry_after_seconds(),
        }
    }
}

#[derive(Debug)]
enum AdmissionSlot {
    Global,
    Pool(String),
    Provider(String),
}

#[derive(Debug)]
pub struct AdmissionGuard {
    controller: Arc<AdmissionController>,
    slots: Vec<AdmissionSlot>,
    active: bool,
}

impl AdmissionGuard {
    fn new(controller: Arc<AdmissionController>, slots: Vec<AdmissionSlot>) -> Self {
        Self {
            controller,
            slots,
            active: true,
        }
    }

    pub fn finish(&mut self) {
        if self.active {
            self.controller.release(&self.slots);
            self.active = false;
        }
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

fn decrement_or_remove(values: &mut BTreeMap<String, u64>, key: &str) {
    if let Some(value) = values.get_mut(key) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            values.remove(key);
        }
    }
}
