use std::{collections::BTreeMap, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
};
use serde_json::Value;
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

#[tokio::test]
async fn models_endpoint_lists_provider_models_and_aliases() {
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-backup", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);
    state.model_aliases = Arc::new(BTreeMap::from([(
        "mock-fast".to_string(),
        "mock-fast-v1".to_string(),
    )]));

    let response = app::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["object"], "list");
    let models = json["data"].as_array().unwrap();
    assert!(models.iter().any(|model| {
        model["id"] == "mock-fast-v1"
            && model["object"] == "model"
            && model["owned_by"] == "rustygate"
            && model.get("resolved_model").is_none()
            && model.get("providers").is_none()
    }));
    assert!(models
        .iter()
        .any(|model| { model["id"] == "mock-fast" && model.get("resolved_model").is_none() }));
}

#[tokio::test]
async fn models_endpoint_requires_authentication() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let response = app::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .method("GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
