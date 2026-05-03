use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use lru::LruCache;

use crate::config::RateLimitConfig;

#[async_trait]
pub trait RateLimitBackend: Send + Sync {
    async fn check_global(&self) -> Result<(), u64>;
    async fn check_key(
        &self,
        api_key: &str,
        requests_per_minute: Option<u32>,
        burst_size: Option<u32>,
    ) -> Result<(), u64>;
}

#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<RateLimiterState>>,
}

struct RateLimiterState {
    global_bucket: TokenBucket,
    per_key_buckets: LruCache<String, TokenBucket>,
    per_key_capacity: f64,
    per_key_refill_per_second: f64,
    per_key_evictions: u64,
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
                per_key_buckets: LruCache::new(
                    NonZeroUsize::new(config.max_tracked_keys).expect("validated max_tracked_keys"),
                ),
                per_key_capacity: config.per_key_burst_size as f64,
                per_key_refill_per_second: requests_per_minute_to_refill_rate(
                    config.per_key_requests_per_minute,
                ),
                per_key_evictions: 0,
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

    pub fn per_key_evictions(&self) -> u64 {
        self.state
            .lock()
            .map(|guard| guard.per_key_evictions)
            .unwrap_or_default()
    }

    pub fn check_key(&self, api_key: &str) -> Result<(), u64> {
        self.check_key_with_override(api_key, None, None)
    }

    pub fn check_key_with_override(
        &self,
        api_key: &str,
        requests_per_minute: Option<u32>,
        burst_size: Option<u32>,
    ) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self.state.lock().map_err(|_| 1_u64)?;
        let per_key_capacity = burst_size
            .map(f64::from)
            .unwrap_or(guard.per_key_capacity)
            .max(1.0);
        let per_key_refill_per_second = requests_per_minute
            .map(requests_per_minute_to_refill_rate)
            .unwrap_or(guard.per_key_refill_per_second);
        if !guard.per_key_buckets.contains(api_key)
            && guard.per_key_buckets.len() == guard.per_key_buckets.cap().get()
        {
            guard.per_key_evictions = guard.per_key_evictions.saturating_add(1);
        }
        let per_key_bucket = guard
            .per_key_buckets
            .get_or_insert_mut(api_key.to_string(), || {
                TokenBucket::new(per_key_capacity, per_key_refill_per_second, now)
            });
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
            if !guard.per_key_buckets.contains(api_key)
                && guard.per_key_buckets.len() == guard.per_key_buckets.cap().get()
            {
                guard.per_key_evictions = guard.per_key_evictions.saturating_add(1);
            }
            let per_key_bucket = guard
                .per_key_buckets
                .get_or_insert_mut(api_key.to_string(), || {
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

#[async_trait]
impl RateLimitBackend for RateLimiter {
    async fn check_global(&self) -> Result<(), u64> {
        RateLimiter::check_global(self)
    }

    async fn check_key(
        &self,
        api_key: &str,
        requests_per_minute: Option<u32>,
        burst_size: Option<u32>,
    ) -> Result<(), u64> {
        self.check_key_with_override(api_key, requests_per_minute, burst_size)
    }
}

#[cfg(feature = "redis-backend")]
pub mod redis_backend {
    use std::{
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use redis::{aio::ConnectionManager, Script};
    use tokio::sync::Mutex;

    use crate::config::RateLimitConfig;

    use super::{requests_per_minute_to_refill_rate, RateLimitBackend, RateLimiter};
    use async_trait::async_trait;

    #[derive(Clone)]
    pub struct RedisRateLimitBackend {
        connection: Arc<Mutex<ConnectionManager>>,
        script: Script,
        local_fallback: Option<RateLimiter>,
        global_capacity: u32,
        global_refill_per_ms: f64,
        per_key_capacity: u32,
        per_key_refill_per_ms: f64,
        key_prefix: String,
    }

    impl RedisRateLimitBackend {
        pub async fn connect(
            redis_url: &str,
            config: &RateLimitConfig,
            local_fallback: Option<RateLimiter>,
        ) -> redis::RedisResult<Self> {
            let client = redis::Client::open(redis_url)?;
            let connection = client.get_connection_manager().await?;
            Ok(Self {
                connection: Arc::new(Mutex::new(connection)),
                script: Script::new(REDIS_TOKEN_BUCKET_SCRIPT),
                local_fallback,
                global_capacity: config.global_burst_size,
                global_refill_per_ms: requests_per_minute_to_refill_rate(
                    config.global_requests_per_minute,
                ) / 1_000.0,
                per_key_capacity: config.per_key_burst_size,
                per_key_refill_per_ms: requests_per_minute_to_refill_rate(
                    config.per_key_requests_per_minute,
                ) / 1_000.0,
                key_prefix: "rg:rl".into(),
            })
        }

        async fn check_bucket(
            &self,
            key: String,
            capacity: u32,
            refill_per_ms: f64,
        ) -> redis::RedisResult<Result<(), u64>> {
            let now_ms = now_ms();
            let ttl_ms = 120_000_i64;
            let mut connection = self.connection.lock().await;
            let retry_after_ms: i64 = self
                .script
                .key(key)
                .arg(now_ms as i64)
                .arg(capacity.max(1) as i64)
                .arg(refill_per_ms.max(0.000001))
                .arg(ttl_ms)
                .invoke_async(&mut *connection)
                .await?;
            if retry_after_ms <= 0 {
                Ok(Ok(()))
            } else {
                Ok(Err(((retry_after_ms as u64) / 1_000).max(1)))
            }
        }

        fn fallback_global(&self) -> Result<(), u64> {
            self.local_fallback
                .as_ref()
                .map(RateLimiter::check_global)
                .unwrap_or(Ok(()))
        }

        fn fallback_key(
            &self,
            api_key: &str,
            requests_per_minute: Option<u32>,
            burst_size: Option<u32>,
        ) -> Result<(), u64> {
            self.local_fallback
                .as_ref()
                .map(|local| {
                    local.check_key_with_override(api_key, requests_per_minute, burst_size)
                })
                .unwrap_or(Ok(()))
        }
    }

    #[async_trait]
    impl RateLimitBackend for RedisRateLimitBackend {
        async fn check_global(&self) -> Result<(), u64> {
            match self
                .check_bucket(
                    format!("{}:global", self.key_prefix),
                    self.global_capacity,
                    self.global_refill_per_ms,
                )
                .await
            {
                Ok(result) => result,
                Err(_) => self.fallback_global(),
            }
        }

        async fn check_key(
            &self,
            api_key: &str,
            requests_per_minute: Option<u32>,
            burst_size: Option<u32>,
        ) -> Result<(), u64> {
            let capacity = burst_size.unwrap_or(self.per_key_capacity);
            let refill_per_ms = requests_per_minute
                .map(|rpm| requests_per_minute_to_refill_rate(rpm) / 1_000.0)
                .unwrap_or(self.per_key_refill_per_ms);
            match self
                .check_bucket(
                    format!("{}:key:{}", self.key_prefix, api_key),
                    capacity,
                    refill_per_ms,
                )
                .await
            {
                Ok(result) => result,
                Err(_) => self.fallback_key(api_key, requests_per_minute, burst_size),
            }
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }

    const REDIS_TOKEN_BUCKET_SCRIPT: &str = r#"
local bucket = redis.call('HMGET', KEYS[1], 'tokens', 'ts')
local tokens = tonumber(bucket[1]) or tonumber(ARGV[2])
local last = tonumber(bucket[2]) or tonumber(ARGV[1])
local refill = (tonumber(ARGV[1]) - last) * tonumber(ARGV[3])
tokens = math.min(tonumber(ARGV[2]), tokens + refill)
if tokens < 1 then
  redis.call('HMSET', KEYS[1], 'tokens', tokens, 'ts', ARGV[1])
  redis.call('PEXPIRE', KEYS[1], ARGV[4])
  local deficit = 1 - tokens
  return math.ceil(deficit / tonumber(ARGV[3]))
end
redis.call('HMSET', KEYS[1], 'tokens', tokens - 1, 'ts', ARGV[1])
redis.call('PEXPIRE', KEYS[1], ARGV[4])
return 0
"#;
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
