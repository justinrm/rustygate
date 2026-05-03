use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    providers::{
        mock::MockProvider,
        provider::{ChatProvider, ProviderEntry, ProviderError, ProviderPricing},
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn chat_completion_success_uses_mock_provider() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let response = app::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "mock-fast-v1",
                        "messages": [
                            {"role": "user", "content": "Say hi"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["object"], "chat.completion");
    let request_id = json["id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
    assert_eq!(json["provider"], "mock-primary");
    assert_eq!(json["model"], "mock-fast-v1");
    assert_eq!(json["usage"]["prompt_tokens"], 2);
    assert_eq!(json["usage"]["completion_tokens"], 5);
    assert_eq!(json["usage"]["total_tokens"], 7);
    assert_eq!(json["choices"][0]["message"]["role"], "assistant");
    assert_eq!(
        json["choices"][0]["message"]["content"],
        "Deterministic mock response from mock-primary."
    );
    assert!(json.get("estimated_cost_usd").is_none());
}

#[tokio::test]
async fn chat_completion_uses_secondary_provider_after_retryable_primary_failure() {
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
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-secondary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["provider"], "mock-secondary");
    assert_eq!(
        json["choices"][0]["message"]["content"],
        "Deterministic mock response from mock-secondary."
    );
}

#[tokio::test]
async fn chat_completion_returns_final_failure_after_all_retryable_providers_fail() {
    let state = AppState::from_providers(vec![
        failing_entry(
            "mock-primary",
            "mock-fast-v1",
            1,
            ProviderError::RateLimited,
        ),
        failing_entry(
            "mock-secondary",
            "mock-fast-v1",
            2,
            ProviderError::RateLimited,
        ),
    ]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "provider_rate_limited");
    let request_id = json["error"]["request_id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
}

#[tokio::test]
async fn chat_completion_returns_503_when_no_provider_supports_requested_model() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "unknown-model",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "no_provider_available");
}

#[tokio::test]
async fn chat_completion_stops_on_non_retryable_provider_error() {
    let state = AppState::from_providers(vec![
        failing_entry(
            "mock-primary",
            "mock-fast-v1",
            1,
            ProviderError::AuthenticationFailed,
        ),
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-secondary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);

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

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let provider_stats_response = app
        .oneshot(
            Request::builder()
                .uri("/stats/providers")
                .method("GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["requests_by_provider"]["mock-primary"], 1);
    assert_eq!(json["requests_by_provider"]["mock-secondary"], Value::Null);
}

#[tokio::test]
async fn provider_failure_response_does_not_include_prompt_content() {
    let prompt = "secret prompt text should not leak";
    let state = AppState::from_providers(vec![failing_entry(
        "mock-primary",
        "mock-fast-v1",
        1,
        ProviderError::ProviderBadResponse,
    )]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": prompt}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();

    assert!(!body_text.contains(prompt));
    assert!(body_text.contains("provider failed to process this request"));
}

#[tokio::test]
async fn chat_completion_rejects_malformed_json_with_clean_error() {
    let response = app::router()
        .oneshot(chat_request_raw("{ not valid json"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "request body must be valid JSON matching the chat completion schema"
    );
    let request_id = json["error"]["request_id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
}

#[tokio::test]
async fn chat_completion_rejects_missing_messages_with_clean_error() {
    let response = app::router()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1"
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "request body must be valid JSON matching the chat completion schema"
    );
}

#[tokio::test]
async fn chat_completion_rejects_invalid_role_with_clean_error() {
    let response = app::router()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "developer", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "request body must be valid JSON matching the chat completion schema"
    );
}

#[tokio::test]
async fn chat_completion_rejects_empty_messages() {
    let response = app::router()
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "mock-fast-v1",
                        "messages": []
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "messages must contain at least one item"
    );
    let request_id = json["error"]["request_id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
}

#[tokio::test]
async fn chat_completion_rejects_missing_model() {
    let response = app::router()
        .oneshot(chat_request(json!({
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(json["error"]["message"], "model must be provided");
    let request_id = json["error"]["request_id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
}

#[tokio::test]
async fn chat_completion_rejects_empty_message_content() {
    let response = app::router()
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "mock-fast-v1",
                        "messages": [
                            {"role": "user", "content": "   "}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "message content must not be empty"
    );
    let request_id = json["error"]["request_id"].as_str().unwrap();
    Uuid::parse_str(request_id).unwrap();
}

fn chat_request(body: Value) -> Request<Body> {
    chat_request_raw(body.to_string())
}

fn chat_request_raw(body: impl Into<String>) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.into()))
        .unwrap()
}

fn failing_entry(name: &str, model: &str, priority: u32, error: ProviderError) -> ProviderEntry {
    ProviderEntry {
        priority,
        provider: Arc::new(FailingProvider {
            name: name.to_string(),
            model: model.to_string(),
            error,
        }) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}

#[derive(Debug)]
struct FailingProvider {
    name: String,
    model: String,
    error: ProviderError,
}

#[async_trait]
impl ChatProvider for FailingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_model(&self, model: &str) -> bool {
        self.model == model
    }

    async fn chat_completion(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Err(self.error.clone())
    }
}
