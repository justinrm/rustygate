use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
    routing::health,
};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn ready_reports_unavailable_when_no_providers_are_registered() {
    let response = app::router_with_state(AppState::default())
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn ready_detail_reports_per_provider_status() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    health::probe_once(&state.providers, &state.provider_health).await;

    let response = app::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/ready?detail=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["providers"]["mock-primary"]["status"], "healthy");
}

#[tokio::test]
async fn ready_turns_503_when_all_providers_fail_health_checks() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider {
            name: "mock-primary".into(),
            model: "mock-fast-v1".into(),
            failure_rate: 1.0,
            base_latency_ms: 0,
        }),
        pricing: ProviderPricing::default(),
    }]);
    health::probe_once(&state.providers, &state.provider_health).await;

    let response = app::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let response = rustygate::app::router()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "rustygate");
}

#[tokio::test]
async fn ready_endpoint_returns_ready() {
    let response = rustygate::app::router()
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "not_ready");
}
