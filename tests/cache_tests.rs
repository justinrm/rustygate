use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    cache::response::MemoryResponseCache,
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

#[tokio::test]
async fn identical_non_streaming_request_hits_response_cache() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.response_cache = Some(Arc::new(MemoryResponseCache::new(
        Duration::from_secs(60),
        100,
    )));
    let router = app::router_with_state(state);
    let payload = json!({
        "model": "mock-fast-v1",
        "messages": [{"role": "user", "content": "cache me"}],
        "temperature": 0
    });

    let first = router
        .clone()
        .oneshot(chat_request(payload.clone()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers().get("x-rustygate-cache").unwrap(), "MISS");

    let second = router.oneshot(chat_request(payload)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.headers().get("x-rustygate-cache").unwrap(), "HIT");
}

#[tokio::test]
async fn temperature_above_zero_skips_cache() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.response_cache = Some(Arc::new(MemoryResponseCache::new(
        Duration::from_secs(60),
        100,
    )));
    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "do not cache"}],
            "temperature": 0.7
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-rustygate-cache").unwrap(), "MISS");
}

fn chat_request(payload: Value) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
        .body(Body::from(payload.to_string()))
        .unwrap()
}
