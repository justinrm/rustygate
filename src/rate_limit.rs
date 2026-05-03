use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::config::RateLimitConfig;

#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<RateLimiterState>>,
}

struct RateLimiterState {
    global_bucket: TokenBucket,
    per_key_buckets: HashMap<String, TokenBucket>,
    per_key_capacity: f64,
    per_key_refill_per_second: f64,
}

impl RateLimiter {
    pub fn new(config: &RateLimitConfig) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(RateLimiterState {
                global_bucket: TokenBucket::new(
                    config.global_burst_size as f64,
                    requests_per_minute_to_refill_rate(config.global_requests_per_minute),
                    now,
                ),
                per_key_buckets: HashMap::new(),
                per_key_capacity: config.per_key_burst_size as f64,
                per_key_refill_per_second: requests_per_minute_to_refill_rate(
                    config.per_key_requests_per_minute,
                ),
            })),
        }
    }

    pub fn check_global(&self) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self.state.lock().map_err(|_| 1_u64)?;
        if let Some(retry_after) = guard.global_bucket.wait_duration(now) {
            return Err(retry_after.as_secs().max(1));
        }

        guard.global_bucket.consume_one(now);
        Ok(())
    }

    pub fn check_key(&self, api_key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self.state.lock().map_err(|_| 1_u64)?;
        let per_key_capacity = guard.per_key_capacity;
        let per_key_refill_per_second = guard.per_key_refill_per_second;
        let per_key_bucket = guard
            .per_key_buckets
            .entry(api_key.to_string())
            .or_insert_with(|| TokenBucket::new(per_key_capacity, per_key_refill_per_second, now));
        if let Some(retry_after) = per_key_bucket.wait_duration(now) {
            return Err(retry_after.as_secs().max(1));
        }

        per_key_bucket.consume_one(now);
        Ok(())
    }

    pub fn check(&self, api_key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self.state.lock().map_err(|_| 1_u64)?;
        let per_key_capacity = guard.per_key_capacity;
        let per_key_refill_per_second = guard.per_key_refill_per_second;
        let key_wait = {
            let per_key_bucket = guard
                .per_key_buckets
                .entry(api_key.to_string())
                .or_insert_with(|| {
                    TokenBucket::new(per_key_capacity, per_key_refill_per_second, now)
                });
            per_key_bucket.wait_duration(now)
        };
        let global_wait = guard.global_bucket.wait_duration(now);
        if let Some(retry_after) = max_wait(global_wait, key_wait) {
            return Err(retry_after.as_secs().max(1));
        }

        guard.global_bucket.consume_one(now);
        if let Some(per_key_bucket) = guard.per_key_buckets.get_mut(api_key) {
            per_key_bucket.consume_one(now);
        }
        Ok(())
    }
}

fn requests_per_minute_to_refill_rate(requests_per_minute: u32) -> f64 {
    (requests_per_minute.max(1) as f64) / 60.0
}

fn max_wait(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_second: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, refill_per_second: f64, now: Instant) -> Self {
        Self {
            capacity: capacity.max(1.0),
            tokens: capacity.max(1.0),
            refill_per_second: refill_per_second.max(0.1),
            last_refill: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed_seconds = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed_seconds * self.refill_per_second).min(self.capacity);
        self.last_refill = now;
    }

    fn wait_duration(&mut self, now: Instant) -> Option<Duration> {
        self.refill(now);
        if self.tokens >= 1.0 {
            None
        } else {
            let deficit = 1.0 - self.tokens;
            Some(Duration::from_secs_f64(
                (deficit / self.refill_per_second).max(0.001),
            ))
        }
    }

    fn consume_one(&mut self, now: Instant) {
        self.refill(now);
        self.tokens = (self.tokens - 1.0).max(0.0);
    }
}
