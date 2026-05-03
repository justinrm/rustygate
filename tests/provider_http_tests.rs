use reqwest::Client;
use rustygate::{
    models::chat::{ChatCompletionRequest, ChatMessage, ChatRole},
    providers::{
        anthropic::AnthropicProvider,
        openai_compatible::OpenAiCompatibleProvider,
        provider::{ChatProvider, ProviderError},
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
        }],
        temperature: Some(0.2),
        max_tokens: Some(64),
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

    let provider = OpenAiCompatibleProvider {
        name: "openai-primary".to_string(),
        model: "gpt-4o-mini".to_string(),
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        client: Client::new(),
    };
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

    let provider = OpenAiCompatibleProvider {
        name: "openai-primary".to_string(),
        model: "gpt-4o-mini".to_string(),
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        client: Client::new(),
    };
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

    let provider = AnthropicProvider {
        name: "anthropic-primary".to_string(),
        model: "claude-3-5-sonnet-latest".to_string(),
        base_url: server.uri(),
        api_key: "anthropic-key".to_string(),
        client: Client::new(),
    };
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

    let provider = AnthropicProvider {
        name: "anthropic-primary".to_string(),
        model: "claude-3-5-sonnet-latest".to_string(),
        base_url: server.uri(),
        api_key: "anthropic-key".to_string(),
        client: Client::new(),
    };
    let error = provider
        .chat_completion(request("claude-3-5-sonnet-latest"))
        .await
        .expect_err("provider should map auth response to provider error");

    assert!(matches!(error, ProviderError::AuthenticationFailed));
}
