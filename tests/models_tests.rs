use std::{collections::BTreeMap, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    config::ModelPoolConfig,
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
    routing::model_pools::ModelPoolIndex,
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
async fn models_endpoint_prefers_public_pool_ids_over_internal_replica_models() {
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: Arc::new(MockProvider::new("replica-a", "internal-replica-a")),
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("replica-b", "internal-replica-b")),
            pricing: ProviderPricing::default(),
        },
    ]);
    state.model_pools = Arc::new(ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "mock-fast-pool".to_string(),
        aliases: vec!["mock-fast".to_string()],
        routing_policy: None,
        members: vec!["replica-a".to_string(), "replica-b".to_string()],
        max_in_flight: None,
    }]));

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
    let model_ids = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    assert!(model_ids.contains(&"mock-fast-pool".to_string()));
    assert!(model_ids.contains(&"mock-fast".to_string()));
    assert!(!model_ids.contains(&"internal-replica-a".to_string()));
    assert!(!model_ids.contains(&"internal-replica-b".to_string()));
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
