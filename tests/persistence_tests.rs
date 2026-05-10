use std::{fs, path::PathBuf, sync::Arc};

use axum::{body::to_bytes, http::StatusCode};
use rustygate::{
    app::{self, AppState},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
    storage::sqlite::SqliteRequestLogStore,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

mod common;

use common::{authenticated_get, chat_request, mock_provider_entry};

#[tokio::test]
async fn sqlite_persists_successful_request_without_prompt_content() {
    let (store, database_path) = temp_store().await;
    let store = Arc::new(store);
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing {
            cost_per_1k_input_tokens: 1.0,
            cost_per_1k_output_tokens: 2.0,
        },
    }])
    .with_request_log_store(store.clone());

    let app = app::router_with_state(state);
    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "do not store this prompt"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(store.count_request_logs().await.unwrap(), 1);
    assert_eq!(store.count_provider_attempts().await.unwrap(), 1);
    assert_eq!(store.count_logs_with_prompt_content().await.unwrap(), 0);

    let stats_response = app.oneshot(authenticated_get("/stats")).await.unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(stats["total_requests"], 1);
    assert_eq!(stats["successful_requests"], 1);
    assert_eq!(stats["failed_requests"], 0);
    assert_eq!(stats["total_provider_attempts"], 1);
    assert_eq!(stats["estimated_prompt_tokens"], 5);
    assert_eq!(stats["estimated_completion_tokens"], 5);
    assert_eq!(stats["estimated_total_tokens"], 10);

    let _ = fs::remove_file(database_path);
}

#[tokio::test]
async fn sqlite_persists_fallback_provider_attempts() {
    let (store, database_path) = temp_store().await;
    let store = Arc::new(store);
    let state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: Arc::new(MockProvider {
                name: "mock-primary".into(),
                model: "mock-fast-v1".into(),
                failure_rate: 1.0,
                base_latency_ms: 0,
            }),
            pricing: ProviderPricing::default(),
        },
        mock_provider_entry("mock-secondary", "mock-fast-v1", 2),
    ])
    .with_request_log_store(store.clone());

    let app = app::router_with_state(state);
    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(store.count_request_logs().await.unwrap(), 1);
    assert_eq!(store.count_provider_attempts().await.unwrap(), 2);

    let provider_stats_response = app
        .oneshot(authenticated_get("/stats/providers"))
        .await
        .unwrap();
    assert_eq!(provider_stats_response.status(), StatusCode::OK);
    let body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(stats["requests_by_provider"]["mock-primary"], 1);
    assert_eq!(stats["errors_by_provider"]["mock-primary"], 1);
    assert_eq!(stats["requests_by_provider"]["mock-secondary"], 1);
    assert_eq!(stats["successes_by_provider"]["mock-secondary"], 1);
    assert_eq!(stats["fallback_attempts_by_provider"]["mock-secondary"], 1);

    let _ = fs::remove_file(database_path);
}

async fn temp_store() -> (SqliteRequestLogStore, PathBuf) {
    let database_path =
        std::env::temp_dir().join(format!("rustygate-persistence-test-{}.db", Uuid::new_v4()));
    let database_url = format!("sqlite://{}", database_path.display());
    let store = SqliteRequestLogStore::connect(&database_url).await.unwrap();

    (store, database_path)
}
