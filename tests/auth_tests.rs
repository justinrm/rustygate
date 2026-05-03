use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    auth::keys::{KeyLimits, KeyRole, SqliteKeyStore},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn valid_sqlite_key_passes_and_role_mismatch_is_forbidden() {
    let store = SqliteKeyStore::connect(&temp_db_url()).await.unwrap();
    let generated = store
        .create_key("inference", KeyRole::Inference, KeyLimits::default(), true)
        .await
        .unwrap();
    let mut state = app_state();
    state.key_store = Arc::new(store);
    let router = app::router_with_state(state);

    let ok = router
        .clone()
        .oneshot(chat_request(&generated.raw_key, chat_payload()))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let forbidden = router
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", generated.raw_key),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoked_key_is_rejected() {
    let store = SqliteKeyStore::connect(&temp_db_url()).await.unwrap();
    let generated = store
        .create_key("admin", KeyRole::Admin, KeyLimits::default(), true)
        .await
        .unwrap();
    store.revoke_key(&generated.id).await.unwrap();

    let mut state = app_state();
    state.key_store = Arc::new(store);
    let response = app::router_with_state(state)
        .oneshot(chat_request(&generated.raw_key, chat_payload()))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn daily_token_quota_rejects_after_usage_is_recorded() {
    let store = SqliteKeyStore::connect(&temp_db_url()).await.unwrap();
    let generated = store
        .create_key(
            "tiny quota",
            KeyRole::Admin,
            KeyLimits {
                requests_per_minute: None,
                daily_token_quota: Some(1),
                daily_cost_quota_usd: None,
            },
            true,
        )
        .await
        .unwrap();
    let mut state = app_state();
    state.key_store = Arc::new(store);
    let router = app::router_with_state(state);

    let first = router
        .clone()
        .oneshot(chat_request(&generated.raw_key, chat_payload()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(10)).await;
    let second = router
        .oneshot(chat_request(&generated.raw_key, chat_payload()))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn argon2_hash_roundtrip() {
    let hash = rustygate::auth::keys::hash_key("rgk_prefix_secret").unwrap();
    assert!(rustygate::auth::keys::verify_key("rgk_prefix_secret", &hash).unwrap());
    assert!(!rustygate::auth::keys::verify_key("rgk_prefix_wrong", &hash).unwrap());
}

fn app_state() -> AppState {
    AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }])
}

fn chat_payload() -> Value {
    json!({
        "model": "mock-fast-v1",
        "messages": [{"role": "user", "content": "hello"}]
    })
}

fn chat_request(api_key: &str, payload: Value) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
        .body(Body::from(payload.to_string()))
        .unwrap()
}

fn temp_db_url() -> String {
    let path = std::env::temp_dir().join(format!("rustygate-auth-{}.db", Uuid::new_v4()));
    format!("sqlite://{}", path.display())
}
