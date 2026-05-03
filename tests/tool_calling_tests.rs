use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    models::chat::{ChatCompletionRequest, ToolChoice, ToolChoiceFunction},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

#[tokio::test]
async fn mock_provider_returns_deterministic_tool_call() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "weather in Austin"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "parameters": {"type": "object"}
                }
            }],
            "tool_choice": "required"
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
}

#[test]
fn tool_choice_referencing_missing_function_is_rejected() {
    let request: ChatCompletionRequest = serde_json::from_value(json!({
        "model": "mock-fast-v1",
        "messages": [{"role": "user", "content": "hello"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "known_tool",
                "parameters": {"type": "object"}
            }
        }],
        "tool_choice": {"type": "function", "function": {"name": "missing_tool"}}
    }))
    .unwrap();

    let error = request.validate(None, &Default::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("tool_choice references undefined tool"));
}

#[test]
fn structured_tool_choice_deserializes_modes_and_functions() {
    let required: ToolChoice = serde_json::from_value(json!("required")).unwrap();
    assert!(matches!(required, ToolChoice::Mode(_)));

    let function: ToolChoice =
        serde_json::from_value(json!({"type": "function", "function": {"name": "lookup"}}))
            .unwrap();
    assert!(matches!(
        function,
        ToolChoice::Function {
            function: ToolChoiceFunction { .. },
            ..
        }
    ));
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
