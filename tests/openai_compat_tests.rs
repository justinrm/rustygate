use std::sync::Arc;

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
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

#[tokio::test]
async fn responses_endpoint_returns_openai_shaped_response() {
    let response = test_app()
        .oneshot(post_json(
            "/v1/responses",
            json!({
                "model": "mock-fast-v1",
                "input": "Say hi"
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["object"], "response");
    assert!(json["id"].as_str().unwrap().starts_with("resp-"));
    assert_eq!(json["status"], "completed");
    assert_eq!(json["output"][0]["type"], "message");
    assert_eq!(json["output"][0]["content"][0]["type"], "output_text");
    assert!(json["usage"]["total_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn responses_endpoint_streams_response_events() {
    let response = test_app()
        .oneshot(post_json(
            "/v1/responses",
            json!({
                "model": "mock-fast-v1",
                "stream": true,
                "input": "Say hi"
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("response.output_text.delta"));
    assert!(body_text.contains("response.completed"));
    assert!(body_text.contains("[DONE]"));
}

#[tokio::test]
async fn embeddings_endpoint_returns_list_shape() {
    let response = test_app()
        .oneshot(post_json(
            "/v1/embeddings",
            json!({
                "model": "text-embedding-3-small",
                "input": ["one", "two"]
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "list");
    assert_eq!(json["data"].as_array().unwrap().len(), 2);
    assert_eq!(json["data"][0]["object"], "embedding");
}

#[tokio::test]
async fn resource_endpoints_return_openai_shapes() {
    let app = test_app();
    let files = app
        .clone()
        .oneshot(authenticated_get("/v1/files"))
        .await
        .unwrap();
    assert_eq!(files.status(), StatusCode::OK);
    let body = to_bytes(files.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "list");

    let batch = app
        .oneshot(post_json(
            "/v1/batches",
            json!({
                "endpoint": "/v1/responses",
                "completion_window": "24h",
                "input_file_id": "file-test"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(batch.status(), StatusCode::OK);
    let body = to_bytes(batch.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "batch");
    assert!(json["id"].as_str().unwrap().starts_with("batch-"));
}

#[tokio::test]
async fn realtime_session_endpoint_returns_client_secret() {
    let response = test_app()
        .oneshot(post_json(
            "/v1/realtime/sessions",
            json!({ "model": "gpt-4o-realtime-preview" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "realtime.session");
    assert!(json["client_secret"]["value"]
        .as_str()
        .unwrap()
        .starts_with("rt-"));
}

fn test_app() -> axum::Router {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    app::router_with_state(state)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authenticated_get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
        .body(Body::empty())
        .unwrap()
}
