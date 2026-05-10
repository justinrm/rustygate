use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use futures_util::StreamExt;
use rustygate::{
    app::{self, AppState},
    config::{AdmissionConfig, RouteExposureConfig},
    models::chat::{
        ChatCompletionChunkResponse, ChatCompletionRequest, ChatCompletionResponse, ChatDelta,
        ChatRole,
    },
    providers::provider::{
        ChatProvider, ProviderEntry, ProviderError, ProviderPricing, ProviderStream,
        ProviderStreamContext, ProviderStreamEvent,
    },
    routing::admission::{AdmissionController, AdmissionLimits},
};
use serde_json::{json, Value};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

mod common;

use common::{authenticated_json_post, mock_provider_entry};

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
async fn responses_stream_times_out_when_provider_stalls_after_first_chunk() {
    let mut state = AppState::from_providers(vec![stream_provider_entry(
        "responses-stall",
        "mock-fast-v1",
        StreamMode::StallAfterFirstChunk,
    )]);
    state.stream_idle_timeout = Duration::from_millis(20);
    let app = app::router_with_state(state);

    let response = app
        .clone()
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
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("response.output_text.delta"));
    assert!(body_text.contains("provider timed out while handling this request"));
    assert!(!body_text.contains("response.completed"));
    assert!(!body_text.contains("[DONE]"));

    let stats_response = app
        .oneshot(common::authenticated_get("/stats"))
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(stats_json["request_errors_by_category"]["timeout"], 1);
    assert_eq!(stats_json["stream_outcomes_by_outcome"]["idle_timeout"], 1);
    assert_eq!(stats_json["in_flight_requests"], 0);
}

#[tokio::test]
async fn responses_stream_treats_missing_completed_event_as_failure() {
    let app = app::router_with_state(AppState::from_providers(vec![stream_provider_entry(
        "responses-incomplete",
        "mock-fast-v1",
        StreamMode::EndAfterFirstChunk,
    )]));

    let response = app
        .clone()
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
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("response.output_text.delta"));
    assert!(body_text.contains("\"error\""));
    assert!(!body_text.contains("response.completed"));
    assert!(!body_text.contains("[DONE]"));

    let stats_response = app
        .oneshot(common::authenticated_get("/stats"))
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(
        stats_json["request_errors_by_category"]["provider_bad_response"],
        1
    );
    assert_eq!(stats_json["stream_outcomes_by_outcome"]["incomplete"], 1);
}

#[tokio::test]
async fn responses_endpoint_applies_admission_token_budget() {
    let mut state =
        AppState::from_providers(vec![mock_provider_entry("mock-primary", "mock-fast-v1", 1)]);
    state.admission = AdmissionController::new(AdmissionLimits::from_config(
        &AdmissionConfig {
            max_estimated_prompt_tokens: Some(1),
            ..AdmissionConfig::default()
        },
        &[],
        &[],
    ));

    let request_id = Uuid::new_v4();
    let response = app::router_with_state(state)
        .oneshot(post_json_with_request_id(
            "/v1/responses",
            json!({
                "model": "mock-fast-v1",
                "input": "two words"
            }),
            request_id,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "estimated prompt token limit exceeded"
    );
    assert_eq!(json["error"]["request_id"], request_id.to_string());
}

#[tokio::test]
async fn chat_validation_error_uses_incoming_request_id() {
    let request_id = Uuid::new_v4();
    let response = test_app()
        .oneshot(post_json_with_request_id(
            "/v1/chat/completions",
            json!({
                "messages": [{"role": "user", "content": "hello"}]
            }),
            request_id,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(json["error"]["request_id"], request_id.to_string());
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
async fn placeholder_compat_routes_can_be_disabled() {
    let mut state =
        AppState::from_providers(vec![mock_provider_entry("mock-primary", "mock-fast-v1", 1)]);
    state.route_exposure = RouteExposureConfig {
        placeholder_compat_routes: false,
    };
    let app = app::router_with_state(state);

    let embeddings = app
        .clone()
        .oneshot(post_json(
            "/v1/embeddings",
            json!({
                "model": "text-embedding-3-small",
                "input": "hello"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(embeddings.status(), StatusCode::NOT_FOUND);

    let responses = app
        .clone()
        .oneshot(post_json(
            "/v1/responses",
            json!({
                "model": "mock-fast-v1",
                "input": "Say hi"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(responses.status(), StatusCode::OK);

    let chat = app
        .clone()
        .oneshot(post_json(
            "/v1/chat/completions",
            json!({
                "model": "mock-fast-v1",
                "messages": [{"role": "user", "content": "hello"}]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);

    let models = app
        .clone()
        .oneshot(authenticated_get("/v1/models"))
        .await
        .unwrap();
    assert_eq!(models.status(), StatusCode::OK);

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let ready = app
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(ready.status(), StatusCode::NOT_FOUND);
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
    let state =
        AppState::from_providers(vec![mock_provider_entry("mock-primary", "mock-fast-v1", 1)]);
    app::router_with_state(state)
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    authenticated_json_post(uri, body)
}

fn post_json_with_request_id(uri: &str, body: Value, request_id: Uuid) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", common::TEST_GATEWAY_KEY),
        )
        .header("x-request-id", request_id.to_string())
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authenticated_get(uri: &str) -> Request<Body> {
    common::authenticated_get(uri)
}

fn stream_provider_entry(name: &str, model: &str, mode: StreamMode) -> ProviderEntry {
    ProviderEntry {
        priority: 1,
        provider: Arc::new(TestStreamProvider {
            name: name.to_string(),
            model: model.to_string(),
            mode,
        }) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}

#[derive(Debug, Clone, Copy)]
enum StreamMode {
    StallAfterFirstChunk,
    EndAfterFirstChunk,
}

#[derive(Debug)]
struct TestStreamProvider {
    name: String,
    model: String,
    mode: StreamMode,
}

#[async_trait]
impl ChatProvider for TestStreamProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn supports_model(&self, model: &str) -> bool {
        self.model == model
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Ok(ChatCompletionResponse::placeholder(
            Uuid::new_v4(),
            request.model.unwrap_or_else(|| self.model.clone()),
            self.name.clone(),
        ))
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        let model = request.model.unwrap_or_else(|| self.model.clone());
        let model_for_stream = model.clone();
        let response_id = Uuid::new_v4();
        let created = OffsetDateTime::now_utc().unix_timestamp();
        let provider_name = self.name.clone();
        let mode = self.mode;
        let stream = async_stream::try_stream! {
            yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                response_id,
                created,
                model_for_stream,
                provider_name,
                0,
                ChatDelta {
                    role: Some(ChatRole::Assistant),
                    content: Some("partial ".to_string()),
                    tool_calls: None,
                },
                None,
            ));

            match mode {
                StreamMode::StallAfterFirstChunk => std::future::pending::<()>().await,
                StreamMode::EndAfterFirstChunk => {}
            }
        }
        .boxed();

        Ok((
            ProviderStreamContext {
                response_id,
                created,
                model,
            },
            stream,
        ))
    }
}
