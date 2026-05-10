use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use rustygate::{
    app::{self, AppState},
    cache::response::{cache_key_for_request, MemoryResponseCache},
    models::chat::{ChatCompletionRequest, ChatMessage, ChatRole},
};
use serde_json::json;
use tower::ServiceExt;

mod common;

use common::{chat_request, mock_provider_entry};

#[tokio::test]
async fn identical_non_streaming_request_hits_response_cache() {
    let mut state =
        AppState::from_providers(vec![mock_provider_entry("mock-primary", "mock-fast-v1", 1)]);
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
    let mut state =
        AppState::from_providers(vec![mock_provider_entry("mock-primary", "mock-fast-v1", 1)]);
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

#[test]
fn cache_key_metadata_does_not_include_prompt_text() {
    let prompt = "secret prompt text should not appear in cache metadata";
    let request = ChatCompletionRequest {
        model: Some("mock-fast-v1".into()),
        messages: vec![ChatMessage {
            role: ChatRole::System,
            content: prompt.into(),
            tool_calls: None,
            tool_call_id: None,
        }],
        temperature: Some(0.0),
        max_tokens: None,
        stream: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        response_format: None,
    };

    let cache_key = cache_key_for_request(&request).expect("cacheable request");

    assert!(!cache_key.as_str().contains(prompt));
    assert_eq!(cache_key.as_str().len(), 64);
}
