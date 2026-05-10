use futures_util::StreamExt;
use reqwest::Client;
use rustygate::{
    models::chat::{ChatCompletionRequest, ChatMessage, ChatRole},
    providers::{
        anthropic::AnthropicProvider,
        openai_compatible::OpenAiCompatibleProvider,
        provider::{ChatProvider, ProviderError, ProviderStreamEvent},
    },
};
use serde_json::json;
use wiremock::{
    matchers::{body_partial_json, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn request(model: &str) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: Some(model.to_string()),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: "Say hi".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }],
        temperature: Some(0.2),
        max_tokens: Some(64),
        stream: None,
        tools: None,
        tool_choice: None,
        parallel_tool_calls: None,
        response_format: None,
    }
}

fn openai_provider(base_url: String) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider {
        name: "openai-primary".to_string(),
        model: "gpt-4o-mini".to_string(),
        base_url,
        api_key: "test-key".to_string(),
        client: Client::new(),
    }
}

fn anthropic_provider(base_url: String) -> AnthropicProvider {
    AnthropicProvider {
        name: "anthropic-primary".to_string(),
        model: "claude-3-5-sonnet-latest".to_string(),
        base_url,
        api_key: "anthropic-key".to_string(),
        client: Client::new(),
    }
}

#[tokio::test]
async fn openai_provider_maps_successful_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .and(body_partial_json(json!({
            "model": "gpt-4o-mini"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl_123",
            "created": 1710000000_i64,
            "model": "gpt-4o-mini",
            "choices": [{
                "message": {"role": "assistant", "content": "Hello from OpenAI"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
        })))
        .mount(&server)
        .await;

    let provider = openai_provider(server.uri());
    let response = provider
        .chat_completion(request("gpt-4o-mini"))
        .await
        .expect("provider call should succeed");

    assert_eq!(response.provider, "openai-primary");
    assert_eq!(response.model, "gpt-4o-mini");
    assert_eq!(response.choices[0].message.content, "Hello from OpenAI");
    assert_eq!(response.usage.total_tokens, 7);
}

#[tokio::test]
async fn openai_provider_maps_rate_limit_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let provider = openai_provider(server.uri());
    let error = provider
        .chat_completion(request("gpt-4o-mini"))
        .await
        .expect_err("provider should return mapped rate-limit error");

    assert!(matches!(error, ProviderError::RateLimited));
}

#[tokio::test]
async fn anthropic_provider_maps_successful_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "anthropic-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_partial_json(json!({
            "model": "claude-3-5-sonnet-latest"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_123",
            "model": "claude-3-5-sonnet-latest",
            "content": [{"type": "text", "text": "Hello from Anthropic"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 8, "output_tokens": 12}
        })))
        .mount(&server)
        .await;

    let provider = anthropic_provider(server.uri());
    let response = provider
        .chat_completion(request("claude-3-5-sonnet-latest"))
        .await
        .expect("provider call should succeed");

    assert_eq!(response.provider, "anthropic-primary");
    assert_eq!(response.model, "claude-3-5-sonnet-latest");
    assert_eq!(response.choices[0].message.content, "Hello from Anthropic");
    assert_eq!(response.usage.prompt_tokens, 8);
    assert_eq!(response.usage.completion_tokens, 12);
}

#[tokio::test]
async fn anthropic_provider_maps_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let provider = anthropic_provider(server.uri());
    let error = provider
        .chat_completion(request("claude-3-5-sonnet-latest"))
        .await
        .expect_err("provider should map auth response to provider error");

    assert!(matches!(error, ProviderError::AuthenticationFailed));
}

#[tokio::test]
async fn openai_provider_streams_incremental_chunks() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"id\":\"chatcmpl_123\",\"created\":1710000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello \"}}]}\n\n",
        "data: {\"id\":\"chatcmpl_123\",\"created\":1710000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = openai_provider(server.uri());

    let (_context, mut stream) = provider
        .chat_completion_stream(ChatCompletionRequest {
            stream: Some(true),
            ..request("gpt-4o-mini")
        })
        .await
        .expect("stream should start");

    let mut chunks = Vec::new();
    let mut usage_total = 0;
    while let Some(event) = stream.next().await {
        match event.expect("stream event should parse") {
            ProviderStreamEvent::Chunk(chunk) => chunks.push(chunk),
            ProviderStreamEvent::Completed { usage } => {
                usage_total = usage.total_tokens;
            }
        }
    }

    assert_eq!(chunks.len(), 2);
    assert_eq!(
        chunks[0].choices[0].delta.content.as_deref(),
        Some("Hello ")
    );
    assert_eq!(chunks[1].choices[0].delta.content.as_deref(), Some("world"));
    assert_eq!(usage_total, 5);
}

#[tokio::test]
async fn openai_provider_rejects_oversized_sse_events() {
    let server = MockServer::start().await;
    let body = format!("data: {}\n\n", "x".repeat(300 * 1024));
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = openai_provider(server.uri());

    let (_context, mut stream) = provider
        .chat_completion_stream(ChatCompletionRequest {
            stream: Some(true),
            ..request("gpt-4o-mini")
        })
        .await
        .expect("stream should start");
    let error = stream
        .next()
        .await
        .expect("stream should emit an error")
        .expect_err("oversized event should be rejected");

    assert!(matches!(error, ProviderError::ProviderBadResponse));
}

#[tokio::test]
async fn anthropic_provider_streams_incremental_chunks() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3-5-sonnet-latest\",\"usage\":{\"input_tokens\":8,\"output_tokens\":0}}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"from Anthropic\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"type\":\"message_delta\",\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":8,\"output_tokens\":2}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = anthropic_provider(server.uri());

    let (_context, mut stream) = provider
        .chat_completion_stream(ChatCompletionRequest {
            stream: Some(true),
            ..request("claude-3-5-sonnet-latest")
        })
        .await
        .expect("stream should start");

    let mut text = String::new();
    let mut usage_total = 0;
    while let Some(event) = stream.next().await {
        match event.expect("stream event should parse") {
            ProviderStreamEvent::Chunk(chunk) => {
                if let Some(content) = chunk.choices[0].delta.content.as_deref() {
                    text.push_str(content);
                }
            }
            ProviderStreamEvent::Completed { usage } => {
                usage_total = usage.total_tokens;
            }
        }
    }

    assert_eq!(text, "Hello from Anthropic");
    assert_eq!(usage_total, 10);
}
