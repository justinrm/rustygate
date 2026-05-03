use std::{sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    config::RateLimitConfig,
    models::chat::ChatValidationLimits,
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
    rate_limit::RateLimiter,
};
use serde_json::{json, Value};
use tokio::time::sleep;
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

#[tokio::test]
async fn protected_endpoints_reject_missing_or_invalid_bearer_token() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    let app = app::router_with_state(state);

    let missing_auth = app
        .clone()
        .oneshot(chat_request_without_auth(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello"}]
        })))
        .await
        .unwrap();
    assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(missing_auth.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "unauthorized");

    let invalid_auth = app
        .clone()
        .oneshot(chat_request_with_token(
            json!({
                "model": "mock-fast-v1",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            "wrong-key",
        ))
        .await
        .unwrap();
    assert_eq!(invalid_auth.status(), StatusCode::UNAUTHORIZED);

    let stats_missing_auth = app
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stats_missing_auth.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pre_auth_rate_limit_applies_to_invalid_bearer_attempts() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.rate_limiter = RateLimiter::new(&RateLimitConfig {
        global_requests_per_minute: 1,
        global_burst_size: 1,
        per_key_requests_per_minute: 120,
        per_key_burst_size: 60,
        ..RateLimitConfig::default()
    });
    let app = app::router_with_state(state);

    let first = app
        .clone()
        .oneshot(chat_request_with_token(
            json!({
                "model": "mock-fast-v1",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            "wrong-key",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::UNAUTHORIZED);

    let second = app
        .oneshot(chat_request_with_token(
            json!({
                "model": "mock-fast-v1",
                "messages": [{"role": "user", "content": "hello again"}]
            }),
            "wrong-key",
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn rate_limit_returns_429_and_retry_after_header() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.rate_limiter = RateLimiter::new(&RateLimitConfig {
        global_requests_per_minute: 1,
        global_burst_size: 1,
        per_key_requests_per_minute: 120,
        per_key_burst_size: 60,
        ..RateLimitConfig::default()
    });
    let app = app::router_with_state(state);

    let first = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello"}]
        })))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello again"}]
        })))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key(header::RETRY_AFTER));
    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "gateway_rate_limited");
}

#[tokio::test]
async fn valid_requests_still_use_per_key_rate_limit() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.rate_limiter = RateLimiter::new(&RateLimitConfig {
        global_requests_per_minute: 120,
        global_burst_size: 60,
        per_key_requests_per_minute: 1,
        per_key_burst_size: 1,
        ..RateLimitConfig::default()
    });
    let app = app::router_with_state(state);

    let first = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello"}]
        })))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello again"}]
        })))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn rate_limit_refills_after_waiting_for_token_bucket() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.rate_limiter = RateLimiter::new(&RateLimitConfig {
        global_requests_per_minute: 60,
        global_burst_size: 1,
        per_key_requests_per_minute: 60,
        per_key_burst_size: 1,
        ..RateLimitConfig::default()
    });
    let app = app::router_with_state(state);

    let first = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "hello"}]
        })))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "too soon"}]
        })))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

    sleep(Duration::from_secs(1)).await;

    let third = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "after refill"}]
        })))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
}

#[tokio::test]
async fn chat_request_body_size_limit_rejects_oversized_payload() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.max_chat_body_bytes = 64;

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "this is definitely larger than sixty-four bytes"}]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn responses_request_body_size_limit_rejects_oversized_payload() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.max_chat_body_bytes = 64;

    let response = app::router_with_state(state)
        .oneshot(responses_request(json!({
            "model": "mock-fast-v1",
            "input": "this is definitely larger than sixty-four bytes once wrapped in json"
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn chat_validation_rejects_message_count_and_content_length_limits() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.chat_validation_limits = ChatValidationLimits {
        max_messages_per_request: 1,
        max_message_content_chars: 5,
    };

    let app = app::router_with_state(state);

    let too_many_messages = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "one"},
                {"role": "assistant", "content": "two"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(too_many_messages.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(too_many_messages.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "messages must contain at most 1 items"
    );

    let content_too_long = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "123456"}]
        })))
        .await
        .unwrap();
    assert_eq!(content_too_long.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(content_too_long.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"]["message"],
        "message content must be at most 5 characters"
    );
}

fn chat_request(body: Value) -> Request<Body> {
    chat_request_with_token(body, TEST_GATEWAY_KEY)
}

fn chat_request_with_token(body: Value, token: &str) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn chat_request_without_auth(body: Value) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn responses_request(body: Value) -> Request<Body> {
    Request::builder()
        .uri("/v1/responses")
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}
