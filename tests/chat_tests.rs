use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use futures_util::StreamExt;
use rustygate::{
    app::{self, AppState},
    config::{
        AdmissionConfig, ModelPoolConfig, PrefixAffinityConfig, ProviderConfig, ProviderKind,
        RoutingPolicy,
    },
    models::chat::{
        ChatCompletionChunkResponse, ChatCompletionRequest, ChatCompletionResponse, ChatDelta,
        ChatRole,
    },
    providers::{
        mock::MockProvider,
        provider::{
            ChatProvider, ProviderEntry, ProviderError, ProviderPricing, ProviderStream,
            ProviderStreamContext, ProviderStreamEvent,
        },
    },
    routing::{
        admission::{AdmissionController, AdmissionLimits},
        model_pools::ModelPoolIndex,
        resilience::{
            CircuitBreakerPolicy, ProviderResiliencePolicy, ResilienceRegistry, RetryPolicy,
        },
    },
};
use serde_json::{json, Value};
use time::OffsetDateTime;
use tokio::sync::Notify;
use tower::ServiceExt;
use uuid::Uuid;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

mod common;

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
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
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
    assert!(request_id.starts_with("chatcmpl-"));
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
async fn chat_completion_resolves_configured_model_alias() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.model_aliases = Arc::new(BTreeMap::from([(
        "mock-fast".to_string(),
        "mock-fast-v1".to_string(),
    )]));

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider"], "mock-primary");
    assert_eq!(json["model"], "mock-fast-v1");
}

#[tokio::test]
async fn chat_completion_routes_public_pool_model_to_pool_members() {
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("replica-a", "internal-replica-a")),
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 1,
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
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider"], "replica-b");
    assert_eq!(json["model"], "internal-replica-b");
}

#[tokio::test]
async fn chat_completion_reuses_replica_for_repeated_prefix_affinity() {
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
    state.routing_policy = RoutingPolicy::PrefixAffinity;
    state.prefix_affinity = PrefixAffinityConfig {
        ttl_seconds: 60,
        max_entries: 100,
        load_imbalance_threshold: 2,
        fallback_policy: RoutingPolicy::Priority,
    };
    state.model_pools = Arc::new(ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "mock-fast-pool".to_string(),
        aliases: vec!["mock-fast".to_string()],
        routing_policy: Some(RoutingPolicy::PrefixAffinity),
        members: vec!["replica-a".to_string(), "replica-b".to_string()],
        max_in_flight: None,
    }]));

    let app = app::router_with_state(state);
    let first = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "system", "content": "You are a terse assistant for account summaries."},
                {"role": "user", "content": "Summarize account A"}
            ]
        })))
        .await
        .unwrap();
    let first_body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
    let first_json: Value = serde_json::from_slice(&first_body).unwrap();
    let selected_provider = first_json["provider"].as_str().unwrap().to_string();

    let second = app
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "system", "content": "You are a terse assistant for account summaries."},
                {"role": "user", "content": "Summarize account B"}
            ]
        })))
        .await
        .unwrap();
    let second_body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    let second_json: Value = serde_json::from_slice(&second_body).unwrap();

    assert_eq!(first_json["object"], "chat.completion");
    assert_eq!(second_json["provider"], selected_provider);
}

#[tokio::test]
async fn chat_completion_without_stable_prefix_uses_prefix_affinity_fallback_policy() {
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
    state.routing_policy = RoutingPolicy::PrefixAffinity;
    state.prefix_affinity = PrefixAffinityConfig {
        ttl_seconds: 60,
        max_entries: 100,
        load_imbalance_threshold: 2,
        fallback_policy: RoutingPolicy::Priority,
    };
    state.model_pools = Arc::new(ModelPoolIndex::from_configs(&[ModelPoolConfig {
        name: "mock-fast-pool".to_string(),
        aliases: vec!["mock-fast".to_string()],
        routing_policy: Some(RoutingPolicy::PrefixAffinity),
        members: vec!["replica-a".to_string(), "replica-b".to_string()],
        max_in_flight: None,
    }]));

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "user", "content": "No reusable prefix here"}
            ]
        })))
        .await
        .unwrap();

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider"], "replica-a");
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
async fn chat_completion_retries_primary_before_fallback() {
    let primary_calls = Arc::new(AtomicUsize::new(0));
    let primary = Arc::new(AlwaysFailingCountedProvider::new(
        "mock-primary",
        "mock-fast-v1",
        ProviderError::Timeout,
        primary_calls.clone(),
    ));
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: primary,
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-secondary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);
    let mut provider_policies = HashMap::new();
    provider_policies.insert(
        "mock-primary".to_string(),
        ProviderResiliencePolicy {
            retry: RetryPolicy {
                max_retries: 2,
                initial_backoff_ms: 0,
                max_backoff_ms: 0,
                jitter_ms: 0,
            },
            breaker: CircuitBreakerPolicy {
                failure_threshold: 10,
                open_duration_ms: 60_000,
                half_open_max_probes: 1,
            },
            ..ProviderResiliencePolicy::default()
        },
    );
    state.resilience = Arc::new(ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        provider_policies,
        &state.provider_names(),
    ));

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
    assert_eq!(primary_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn chat_completion_skips_open_circuit_and_reenters_after_recovery_probe() {
    let fail_once_provider = Arc::new(FailOnceThenSucceedProvider::new(
        "mock-primary",
        "mock-fast-v1",
        ProviderError::Timeout,
    ));
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: fail_once_provider.clone(),
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-secondary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);
    let mut provider_policies = HashMap::new();
    provider_policies.insert(
        "mock-primary".to_string(),
        ProviderResiliencePolicy {
            retry: RetryPolicy {
                max_retries: 0,
                initial_backoff_ms: 0,
                max_backoff_ms: 0,
                jitter_ms: 0,
            },
            breaker: CircuitBreakerPolicy {
                failure_threshold: 1,
                open_duration_ms: 0,
                half_open_max_probes: 1,
            },
            ..ProviderResiliencePolicy::default()
        },
    );
    state.resilience = Arc::new(ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        provider_policies,
        &state.provider_names(),
    ));
    let app = app::router_with_state(state);

    let first_response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "First"}]
        })))
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_body = to_bytes(first_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: Value = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first_json["provider"], "mock-secondary");

    let second_response = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "Second"}]
        })))
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);
    let second_body = to_bytes(second_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["provider"], "mock-primary");
    assert_eq!(fail_once_provider.calls(), 2);
}

#[tokio::test]
async fn chat_completion_skips_provider_while_circuit_is_open() {
    let primary_calls = Arc::new(AtomicUsize::new(0));
    let primary = Arc::new(AlwaysFailingCountedProvider::new(
        "mock-primary",
        "mock-fast-v1",
        ProviderError::Timeout,
        primary_calls.clone(),
    ));
    let mut state = AppState::from_providers(vec![
        ProviderEntry {
            priority: 1,
            provider: primary,
            pricing: ProviderPricing::default(),
        },
        ProviderEntry {
            priority: 2,
            provider: Arc::new(MockProvider::new("mock-secondary", "mock-fast-v1")),
            pricing: ProviderPricing::default(),
        },
    ]);
    let mut provider_policies = HashMap::new();
    provider_policies.insert(
        "mock-primary".to_string(),
        ProviderResiliencePolicy {
            retry: RetryPolicy {
                max_retries: 0,
                initial_backoff_ms: 0,
                max_backoff_ms: 0,
                jitter_ms: 0,
            },
            breaker: CircuitBreakerPolicy {
                failure_threshold: 1,
                open_duration_ms: 60_000,
                half_open_max_probes: 1,
            },
            ..ProviderResiliencePolicy::default()
        },
    );
    state.resilience = Arc::new(ResilienceRegistry::new(
        ProviderResiliencePolicy::default(),
        provider_policies,
        &state.provider_names(),
    ));
    let app = app::router_with_state(state);

    let first = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "First"}]
        })))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(primary_calls.load(Ordering::SeqCst), 1);

    let second = app
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [{"role": "user", "content": "Second"}]
        })))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
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
async fn chat_completion_rejects_global_admission_limit_and_releases() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut state = AppState::from_providers(vec![blocking_entry(
        "mock-primary",
        "mock-fast-v1",
        started.clone(),
        release.clone(),
    )]);
    state.admission = admission_controller(
        AdmissionConfig {
            max_global_in_flight: Some(1),
            retry_after_seconds: 2,
            ..AdmissionConfig::default()
        },
        &[],
        &[],
    );

    let app = app::router_with_state(state);
    let first = tokio::spawn(app.clone().oneshot(chat_request(json!({
        "model": "mock-fast-v1",
        "messages": [
            {"role": "user", "content": "Say hi"}
        ]
    }))));
    started.notified().await;

    let rejected = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi again"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        rejected
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("2")
    );
    let body = to_bytes(rejected.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "admission_rejected");

    release.notify_one();
    let first_response = first.await.unwrap().unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);

    let after_release = tokio::spawn(app.clone().oneshot(chat_request(json!({
        "model": "mock-fast-v1",
        "messages": [
            {"role": "user", "content": "Say hi after release"}
        ]
    }))));
    started.notified().await;
    release.notify_one();
    assert_eq!(
        after_release.await.unwrap().unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn chat_completion_rejects_pool_admission_limit() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let pool_config = ModelPoolConfig {
        name: "mock-fast-pool".to_string(),
        aliases: vec!["mock-fast".to_string()],
        routing_policy: None,
        members: vec!["replica-a".to_string()],
        max_in_flight: Some(1),
    };
    let mut state = AppState::from_providers(vec![blocking_entry(
        "replica-a",
        "internal-replica-a",
        started.clone(),
        release.clone(),
    )]);
    state.model_pools = Arc::new(ModelPoolIndex::from_configs(std::slice::from_ref(
        &pool_config,
    )));
    state.admission = admission_controller(AdmissionConfig::default(), &[], &[pool_config]);

    let app = app::router_with_state(state);
    let first = tokio::spawn(app.clone().oneshot(chat_request(json!({
        "model": "mock-fast",
        "messages": [
            {"role": "user", "content": "Say hi"}
        ]
    }))));
    started.notified().await;

    let rejected = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast",
            "messages": [
                {"role": "user", "content": "Say hi again"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(rejected.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "admission_rejected");
    assert_eq!(
        json["error"]["message"],
        "model pool in-flight limit exceeded"
    );

    release.notify_one();
    assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn chat_completion_rejects_provider_admission_limit() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut state = AppState::from_providers(vec![blocking_entry(
        "mock-primary",
        "mock-fast-v1",
        started.clone(),
        release.clone(),
    )]);
    state.admission = admission_controller(
        AdmissionConfig::default(),
        &[provider_config("mock-primary", "mock-fast-v1", Some(1))],
        &[],
    );

    let app = app::router_with_state(state);
    let first = tokio::spawn(app.clone().oneshot(chat_request(json!({
        "model": "mock-fast-v1",
        "messages": [
            {"role": "user", "content": "Say hi"}
        ]
    }))));
    started.notified().await;

    let rejected = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi again"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(rejected.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "admission_rejected");
    assert_eq!(
        json["error"]["message"],
        "provider in-flight limit exceeded"
    );

    release.notify_one();
    assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn chat_completion_rejects_estimated_token_budgets() {
    let mut prompt_state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    prompt_state.admission = admission_controller(
        AdmissionConfig {
            max_estimated_prompt_tokens: Some(1),
            ..AdmissionConfig::default()
        },
        &[],
        &[],
    );

    let prompt_response = app::router_with_state(prompt_state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "two words"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(prompt_response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(prompt_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "estimated prompt token limit exceeded"
    );

    let mut total_state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    total_state.admission = admission_controller(
        AdmissionConfig {
            max_estimated_total_tokens: Some(3),
            ..AdmissionConfig::default()
        },
        &[],
        &[],
    );

    let total_response = app::router_with_state(total_state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "max_tokens": 3,
            "messages": [
                {"role": "user", "content": "one"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(total_response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(total_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "invalid_request");
    assert_eq!(
        json["error"]["message"],
        "estimated total token limit exceeded"
    );
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
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
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
    assert_eq!(
        json["in_flight_requests_by_provider"]["mock-primary"],
        Value::Null
    );
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
                {"role": "invalid-role", "content": "Say hi"}
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
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
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
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
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

#[tokio::test]
async fn chat_completion_stream_returns_sse_chunks_and_done_marker() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let app = app::router_with_state(state);
    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "stream": true,
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
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
    assert!(body_text.contains("chat.completion.chunk"));
    assert!(body_text.contains("mock-primary"));
    assert!(body_text.contains("[DONE]"));

    let provider_stats_response = app
        .oneshot(
            Request::builder()
                .uri("/stats/providers")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(
        stats_json["in_flight_requests_by_provider"]["mock-primary"],
        Value::Null
    );
    assert!(stats_json["p50_ttft_ms_by_provider"]["mock-primary"]
        .as_f64()
        .is_some());
    assert!(stats_json["p95_ttft_ms_by_provider"]["mock-primary"]
        .as_f64()
        .is_some());
}

#[tokio::test]
async fn chat_stream_falls_back_before_first_chunk() {
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

    let app = app::router_with_state(state);
    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "stream": true,
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("mock-secondary"));
    assert!(!body_text.contains("mock-primary"));

    let provider_stats_response = app
        .oneshot(
            Request::builder()
                .uri("/stats/providers")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(
        stats_json["in_flight_requests_by_provider"]["mock-primary"],
        Value::Null
    );
    assert_eq!(
        stats_json["in_flight_requests_by_provider"]["mock-secondary"],
        Value::Null
    );
}

#[tokio::test]
async fn chat_stream_emits_error_event_after_partial_output() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MidstreamFailingProvider::new(
            "midstream-fail",
            "mock-fast-v1",
            ProviderError::Timeout,
        )),
        pricing: ProviderPricing::default(),
    }]);

    let response = app::router_with_state(state)
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "stream": true,
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("chat.completion.chunk"));
    assert!(body_text.contains("\"error\""));
    assert!(body_text.contains("provider timed out while handling this request"));
    assert_eq!(body_text.matches("\"provider\"").count(), 1);
    assert!(!body_text.contains("[DONE]"));
}

#[tokio::test]
async fn chat_stream_times_out_when_provider_stalls_after_first_chunk() {
    let mut state = AppState::from_providers(vec![stalling_stream_entry(
        "stalling-stream",
        "mock-fast-v1",
    )]);
    state.stream_idle_timeout = Duration::from_millis(20);
    let app = app::router_with_state(state);

    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "stream": true,
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body_text.contains("chat.completion.chunk"));
    assert!(body_text.contains("provider timed out while handling this request"));
    assert!(!body_text.contains("[DONE]"));

    let stats_response = app
        .clone()
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

    let provider_stats_response = app
        .oneshot(common::authenticated_get("/stats/providers"))
        .await
        .unwrap();
    let provider_stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_stats_json: Value = serde_json::from_slice(&provider_stats_body).unwrap();
    assert_eq!(
        provider_stats_json["in_flight_requests_by_provider"]["stalling-stream"],
        Value::Null
    );
}

#[tokio::test]
async fn chat_stream_releases_guards_when_downstream_body_is_dropped() {
    let provider_name = "cancel-stream";
    let provider_config = provider_config(provider_name, "mock-fast-v1", Some(1));
    let mut state =
        AppState::from_providers(vec![stalling_stream_entry(provider_name, "mock-fast-v1")]);
    state.admission = admission_controller(
        AdmissionConfig {
            max_global_in_flight: Some(1),
            ..AdmissionConfig::default()
        },
        &[provider_config],
        &[],
    );
    state.stream_idle_timeout = Duration::from_millis(1_000);
    let app = app::router_with_state(state);

    let response = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "stream": true,
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut body_stream = response.into_body().into_data_stream();
    let first_chunk = body_stream.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first_chunk).contains("chat.completion.chunk"));
    drop(body_stream);
    tokio::time::sleep(Duration::from_millis(10)).await;

    let follow_up = app
        .clone()
        .oneshot(chat_request(json!({
            "model": "mock-fast-v1",
            "messages": [
                {"role": "user", "content": "Say hi"}
            ]
        })))
        .await
        .unwrap();
    assert_eq!(follow_up.status(), StatusCode::OK);

    let stats_response = app
        .oneshot(common::authenticated_get("/stats"))
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(stats_json["in_flight_requests"], 0);
    assert_eq!(stats_json["stream_outcomes_by_outcome"]["cancelled"], 1);
}

fn chat_request(body: Value) -> Request<Body> {
    common::chat_request(body)
}

fn chat_request_raw(body: impl Into<String>) -> Request<Body> {
    Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
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

fn blocking_entry(
    name: &str,
    model: &str,
    started: Arc<Notify>,
    release: Arc<Notify>,
) -> ProviderEntry {
    ProviderEntry {
        priority: 1,
        provider: Arc::new(BlockingProvider {
            name: name.to_string(),
            model: model.to_string(),
            started,
            release,
        }) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}

fn stalling_stream_entry(name: &str, model: &str) -> ProviderEntry {
    ProviderEntry {
        priority: 1,
        provider: Arc::new(StallingStreamProvider {
            name: name.to_string(),
            model: model.to_string(),
        }) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}

fn admission_controller(
    admission: AdmissionConfig,
    providers: &[ProviderConfig],
    model_pools: &[ModelPoolConfig],
) -> Arc<AdmissionController> {
    AdmissionController::new(AdmissionLimits::from_config(
        &admission,
        providers,
        model_pools,
    ))
}

fn provider_config(name: &str, model: &str, max_in_flight: Option<u64>) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        kind: ProviderKind::Mock,
        model: model.to_string(),
        priority: 1,
        failure_rate: 0.0,
        base_latency_ms: 0,
        base_url: None,
        api_key_env: None,
        timeout_ms: None,
        max_retries: None,
        retry_initial_backoff_ms: None,
        retry_max_backoff_ms: None,
        retry_jitter_ms: None,
        circuit_breaker_failure_threshold: None,
        circuit_breaker_open_duration_ms: None,
        circuit_breaker_half_open_max_probes: None,
        max_in_flight,
        cost_per_1k_input_tokens: 0.0,
        cost_per_1k_output_tokens: 0.0,
    }
}

#[derive(Debug)]
struct BlockingProvider {
    name: String,
    model: String,
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[derive(Debug)]
struct StallingStreamProvider {
    name: String,
    model: String,
}

#[async_trait]
impl ChatProvider for StallingStreamProvider {
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
            std::future::pending::<()>().await;
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

#[async_trait]
impl ChatProvider for BlockingProvider {
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
        self.started.notify_one();
        self.release.notified().await;
        Ok(ChatCompletionResponse::placeholder(
            Uuid::new_v4(),
            request.model.unwrap_or_else(|| self.model.clone()),
            self.name.clone(),
        ))
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        Err(ProviderError::ProviderBadResponse)
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

    fn model(&self) -> &str {
        &self.model
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

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        Err(self.error.clone())
    }
}

#[derive(Debug)]
struct MidstreamFailingProvider {
    name: String,
    model: String,
    error: ProviderError,
}

#[derive(Debug)]
struct AlwaysFailingCountedProvider {
    name: String,
    model: String,
    error: ProviderError,
    calls: Arc<AtomicUsize>,
}

impl AlwaysFailingCountedProvider {
    fn new(name: &str, model: &str, error: ProviderError, calls: Arc<AtomicUsize>) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            error,
            calls,
        }
    }
}

#[async_trait]
impl ChatProvider for AlwaysFailingCountedProvider {
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
        _request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }
}

#[derive(Debug)]
struct FailOnceThenSucceedProvider {
    name: String,
    model: String,
    first_error: ProviderError,
    calls: AtomicUsize,
}

impl FailOnceThenSucceedProvider {
    fn new(name: &str, model: &str, first_error: ProviderError) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            first_error,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChatProvider for FailOnceThenSucceedProvider {
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
        let current = self.calls.fetch_add(1, Ordering::SeqCst);
        if current == 0 {
            return Err(self.first_error.clone());
        }

        Ok(ChatCompletionResponse::placeholder(
            Uuid::new_v4(),
            request.model.unwrap_or_else(|| self.model.clone()),
            self.name.clone(),
        ))
    }

    async fn chat_completion_stream(
        &self,
        _request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        Err(ProviderError::ProviderBadResponse)
    }
}

impl MidstreamFailingProvider {
    fn new(name: &str, model: &str, error: ProviderError) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            error,
        }
    }
}

#[async_trait]
impl ChatProvider for MidstreamFailingProvider {
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
        _request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        Err(self.error.clone())
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
        let error = self.error.clone();
        let stream = async_stream::try_stream! {
            yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                response_id,
                created,
                model_for_stream.clone(),
                provider_name,
                0,
                ChatDelta {
                    role: Some(ChatRole::Assistant),
                    content: Some("partial ".to_string()),
                    tool_calls: None,
                },
                None,
            ));
            Err::<(), _>(error)?;
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
