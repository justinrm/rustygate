#![cfg(feature = "redis-backend")]

use rustygate::{
    config::RateLimitConfig,
    rate_limit::{redis_backend::RedisRateLimitBackend, RateLimitBackend, RateLimiter},
};

#[tokio::test]
#[ignore = "requires RUSTYGATE_TEST_REDIS_URL"]
async fn redis_backend_shares_limit_across_instances() {
    let redis_url = std::env::var("RUSTYGATE_TEST_REDIS_URL")
        .expect("RUSTYGATE_TEST_REDIS_URL must point at a disposable Redis instance");
    let config = RateLimitConfig {
        global_requests_per_minute: 1,
        global_burst_size: 1,
        per_key_requests_per_minute: 1,
        per_key_burst_size: 1,
        ..RateLimitConfig::default()
    };
    let first =
        RedisRateLimitBackend::connect(&redis_url, &config, Some(RateLimiter::new(&config)))
            .await
            .unwrap();
    let second =
        RedisRateLimitBackend::connect(&redis_url, &config, Some(RateLimiter::new(&config)))
            .await
            .unwrap();

    first.check_key("shared", None, None).await.unwrap();
    assert!(second.check_key("shared", None, None).await.is_err());
}
