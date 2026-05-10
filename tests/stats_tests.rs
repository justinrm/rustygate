use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use rustygate::{
    app::{self, AppState},
    config::{AdmissionConfig, ModelPoolConfig, PrefixAffinityConfig, RoutingPolicy},
    providers::{
        mock::MockProvider,
        provider::{ProviderEntry, ProviderPricing},
    },
    routing::{
        admission::{AdmissionController, AdmissionLimits},
        model_pools::ModelPoolIndex,
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_GATEWAY_KEY: &str = "test-gateway-key";

mod common;

use common::{authenticated_get, authenticated_json_post};

#[tokio::test]
async fn stats_endpoints_report_success_tokens_and_cost() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing {
            cost_per_1k_input_tokens: 1.0,
            cost_per_1k_output_tokens: 2.0,
        },
    }]);

    let app = app::router_with_state(state);
    let chat_response = app
        .clone()
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
                            {"role": "user", "content": "hello world"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::OK);

    let stats_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();

    assert_eq!(stats_json["total_requests"], 1);
    assert_eq!(stats_json["successful_requests"], 1);
    assert_eq!(stats_json["failed_requests"], 0);
    assert_eq!(stats_json["total_provider_attempts"], 1);
    assert_eq!(stats_json["fallback_attempts"], 0);
    assert_eq!(stats_json["error_rate"], 0.0);
    assert!(stats_json["avg_latency_ms"].as_f64().is_some());
    assert!(stats_json["p95_latency_ms"].as_f64().is_some());
    assert_eq!(stats_json["estimated_prompt_tokens"], 2);
    assert_eq!(stats_json["estimated_completion_tokens"], 5);
    assert_eq!(stats_json["estimated_total_tokens"], 7);
    let input_cost = stats_json["estimated_input_cost_usd"].as_f64().unwrap();
    assert!((input_cost - 0.002).abs() < 1e-9);
    let output_cost = stats_json["estimated_output_cost_usd"].as_f64().unwrap();
    assert!((output_cost - 0.010).abs() < 1e-9);
    let estimated_cost = stats_json["estimated_total_cost_usd"].as_f64().unwrap();
    assert!((estimated_cost - 0.012).abs() < 1e-9);

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
    assert_eq!(provider_stats_response.status(), StatusCode::OK);
    let provider_stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_stats_json: Value = serde_json::from_slice(&provider_stats_body).unwrap();

    assert_eq!(
        provider_stats_json["requests_by_provider"]["mock-primary"],
        1
    );
    assert_eq!(
        provider_stats_json["successes_by_provider"]["mock-primary"],
        1
    );
    assert_eq!(
        provider_stats_json["errors_by_provider"]["mock-primary"],
        Value::Null
    );
    assert_eq!(
        provider_stats_json["fallback_attempts_by_provider"]["mock-primary"],
        Value::Null
    );
    assert!(
        provider_stats_json["avg_latency_ms_by_provider"]["mock-primary"]
            .as_f64()
            .is_some()
    );
    assert!(
        provider_stats_json["p95_latency_ms_by_provider"]["mock-primary"]
            .as_f64()
            .is_some()
    );
    assert_eq!(
        provider_stats_json["in_flight_requests_by_provider"]["mock-primary"],
        Value::Null
    );
    assert_eq!(
        provider_stats_json["circuit_state_by_provider"]["mock-primary"],
        "closed"
    );
}

#[tokio::test]
async fn stats_endpoints_report_provider_failures() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider {
            name: "mock-failing".into(),
            model: "mock-fast-v1".into(),
            failure_rate: 1.0,
            base_latency_ms: 0,
        }),
        pricing: ProviderPricing::default(),
    }]);

    let app = app::router_with_state(state);
    let chat_response = app
        .clone()
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
                            {"role": "user", "content": "hello world"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::BAD_GATEWAY);

    let stats_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();

    assert_eq!(stats_json["total_requests"], 1);
    assert_eq!(stats_json["successful_requests"], 0);
    assert_eq!(stats_json["failed_requests"], 1);
    assert_eq!(stats_json["in_flight_requests"], 0);
    assert_eq!(stats_json["total_provider_attempts"], 1);
    assert_eq!(stats_json["fallback_attempts"], 0);
    assert_eq!(stats_json["error_rate"], 1.0);
    assert_eq!(
        stats_json["request_errors_by_category"]["provider_unavailable"],
        1
    );
    assert!(stats_json["avg_latency_ms"].as_f64().is_some());
    assert!(stats_json["p95_latency_ms"].as_f64().is_some());
    assert_eq!(stats_json["estimated_prompt_tokens"], 0);
    assert_eq!(stats_json["estimated_completion_tokens"], 0);
    assert_eq!(stats_json["estimated_total_tokens"], 0);
    assert_eq!(stats_json["estimated_input_cost_usd"], 0.0);
    assert_eq!(stats_json["estimated_output_cost_usd"], 0.0);
    assert_eq!(stats_json["estimated_total_cost_usd"], 0.0);

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
    assert_eq!(provider_stats_response.status(), StatusCode::OK);
    let provider_stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_stats_json: Value = serde_json::from_slice(&provider_stats_body).unwrap();

    assert_eq!(
        provider_stats_json["requests_by_provider"]["mock-failing"],
        1
    );
    assert_eq!(
        provider_stats_json["successes_by_provider"]["mock-failing"],
        Value::Null
    );
    assert_eq!(provider_stats_json["errors_by_provider"]["mock-failing"], 1);
    assert_eq!(
        provider_stats_json["provider_errors_by_provider_and_category"]["mock-failing"]
            ["provider_unavailable"],
        1
    );
    assert_eq!(
        provider_stats_json["fallback_attempts_by_provider"]["mock-failing"],
        Value::Null
    );
    assert!(
        provider_stats_json["avg_latency_ms_by_provider"]["mock-failing"]
            .as_f64()
            .is_some()
    );
    assert!(
        provider_stats_json["p95_latency_ms_by_provider"]["mock-failing"]
            .as_f64()
            .is_some()
    );
}

#[tokio::test]
async fn metrics_endpoint_exposes_prometheus_scrape_output() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider {
            name: "mock-failing".into(),
            model: "mock-fast-v1".into(),
            failure_rate: 1.0,
            base_latency_ms: 0,
        }),
        pricing: ProviderPricing::default(),
    }]);

    let app = app::router_with_state(state);
    let chat_response = app
        .clone()
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
                            {"role": "user", "content": "hello world"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::BAD_GATEWAY);

    let metrics_response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(metrics_response.status(), StatusCode::OK);
    let content_type = metrics_response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("text/plain; version=0.0.4"));
    let body = to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();

    assert!(body.contains("rustygate_requests_total 1\n"));
    assert!(body.contains("rustygate_requests_failed_total 1\n"));
    assert!(body.contains("rustygate_in_flight_requests 0\n"));
    assert!(body.contains("# HELP rustygate_provider_in_flight_requests"));
    assert!(body.contains("# HELP rustygate_provider_ttft_ms_p50"));
    assert!(body.contains("# HELP rustygate_provider_ttft_ms_p95"));
    assert!(body.contains("# HELP rustygate_provider_queue_pressure"));
    assert!(body.contains("# HELP rustygate_routing_decisions_total"));
    assert!(body.contains("# HELP rustygate_stream_outcomes_total"));
    assert!(body.contains("# HELP rustygate_stream_duration_ms_p95"));
    assert!(body.contains("rustygate_request_errors_total{category=\"provider_unavailable\"} 1\n"));
    assert!(body.contains(
        "rustygate_provider_errors_total{provider=\"mock-failing\",category=\"provider_unavailable\"} 1\n"
    ));
}

#[tokio::test]
async fn stats_and_metrics_report_admission_rejections() {
    let mut state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    state.admission = AdmissionController::new(AdmissionLimits::from_config(
        &AdmissionConfig {
            max_estimated_prompt_tokens: Some(1),
            ..AdmissionConfig::default()
        },
        &[],
        &[],
    ));

    let app = app::router_with_state(state);
    let chat_response = app
        .clone()
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
                            {"role": "user", "content": "two words"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::BAD_REQUEST);

    let stats_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(
        stats_json["admission_rejections_by_reason"]["max_estimated_prompt_tokens"],
        1
    );

    let metrics_response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let metrics_body = to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_text = String::from_utf8(metrics_body.to_vec()).unwrap();
    assert!(metrics_text.contains(
        "rustygate_admission_rejections_total{reason=\"max_estimated_prompt_tokens\"} 1\n"
    ));
}

#[tokio::test]
async fn stats_endpoints_report_provider_attempts_and_fallbacks() {
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
    let chat_response = app
        .clone()
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
                            {"role": "user", "content": "hello world"}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::OK);

    let stats_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();

    assert_eq!(stats_json["total_requests"], 1);
    assert_eq!(stats_json["successful_requests"], 1);
    assert_eq!(stats_json["failed_requests"], 0);
    assert_eq!(stats_json["total_provider_attempts"], 2);
    assert_eq!(stats_json["fallback_attempts"], 1);
    assert_eq!(stats_json["error_rate"], 0.0);
    assert!(stats_json["avg_latency_ms"].as_f64().is_some());
    assert!(stats_json["p95_latency_ms"].as_f64().is_some());

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
    let provider_stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_stats_json: Value = serde_json::from_slice(&provider_stats_body).unwrap();

    assert_eq!(
        provider_stats_json["requests_by_provider"]["mock-primary"],
        1
    );
    assert_eq!(
        provider_stats_json["requests_by_provider"]["mock-secondary"],
        1
    );
    assert_eq!(provider_stats_json["errors_by_provider"]["mock-primary"], 1);
    assert_eq!(
        provider_stats_json["successes_by_provider"]["mock-secondary"],
        1
    );
    assert_eq!(
        provider_stats_json["fallback_attempts_by_provider"]["mock-secondary"],
        1
    );
    assert!(
        provider_stats_json["avg_latency_ms_by_provider"]["mock-primary"]
            .as_f64()
            .is_some()
    );
    assert!(
        provider_stats_json["avg_latency_ms_by_provider"]["mock-secondary"]
            .as_f64()
            .is_some()
    );
    assert!(
        provider_stats_json["p95_latency_ms_by_provider"]["mock-primary"]
            .as_f64()
            .is_some()
    );
    assert!(
        provider_stats_json["p95_latency_ms_by_provider"]["mock-secondary"]
            .as_f64()
            .is_some()
    );
}

#[tokio::test]
async fn stats_count_invalid_chat_requests_without_provider_attempts() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);

    let app = app::router_with_state(state);
    let prompt_needle = "never-log-this-prompt";
    let chat_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/chat/completions")
                .method("POST")
                .header("content-type", "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::from(
                    json!({
                        "model": " ",
                        "messages": [
                            {"role": "user", "content": prompt_needle}
                        ]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_response.status(), StatusCode::BAD_REQUEST);

    let stats_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stats")
                .method("GET")
                .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_text = String::from_utf8(stats_body.to_vec()).unwrap();
    assert!(!stats_text.contains(prompt_needle));
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();

    assert_eq!(stats_json["total_requests"], 1);
    assert_eq!(stats_json["successful_requests"], 0);
    assert_eq!(stats_json["failed_requests"], 1);
    assert_eq!(stats_json["total_provider_attempts"], 0);
    assert_eq!(stats_json["fallback_attempts"], 0);
    assert_eq!(stats_json["error_rate"], 1.0);
    assert!(stats_json["avg_latency_ms"].as_f64().is_some());
    assert!(stats_json["p95_latency_ms"].as_f64().is_some());
    assert_eq!(stats_json["estimated_total_tokens"], 0);
    assert_eq!(stats_json["estimated_total_cost_usd"], 0.0);

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
    let provider_stats_body = to_bytes(provider_stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_stats_text = String::from_utf8(provider_stats_body.to_vec()).unwrap();
    assert!(!provider_stats_text.contains(prompt_needle));
    let provider_stats_json: Value = serde_json::from_slice(&provider_stats_body).unwrap();

    assert_eq!(provider_stats_json["requests_by_provider"], json!({}));
    assert_eq!(provider_stats_json["successes_by_provider"], json!({}));
    assert_eq!(provider_stats_json["errors_by_provider"], json!({}));
    assert_eq!(
        provider_stats_json["fallback_attempts_by_provider"],
        json!({})
    );
    assert_eq!(provider_stats_json["avg_latency_ms_by_provider"], json!({}));
    assert_eq!(provider_stats_json["p95_latency_ms_by_provider"], json!({}));
}

#[tokio::test]
async fn stats_and_metrics_report_prefix_fingerprint_outcomes_without_prompt_text() {
    let state = AppState::from_providers(vec![ProviderEntry {
        priority: 1,
        provider: Arc::new(MockProvider::new("mock-primary", "mock-fast-v1")),
        pricing: ProviderPricing::default(),
    }]);
    let app = app::router_with_state(state);
    let secret_prefix = "secret shared system prompt should not leak";

    let high_confidence_chat = app
        .clone()
        .oneshot(authenticated_json_request(
            "/v1/chat/completions",
            json!({
                "model": "mock-fast-v1",
                "messages": [
                    {"role": "system", "content": secret_prefix},
                    {"role": "user", "content": "volatile suffix A"}
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(high_confidence_chat.status(), StatusCode::OK);

    let high_confidence_response = app
        .clone()
        .oneshot(authenticated_json_request(
            "/v1/responses",
            json!({
                "model": "mock-fast-v1",
                "instructions": secret_prefix,
                "input": "volatile suffix B"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(high_confidence_response.status(), StatusCode::OK);

    let low_confidence_chat = app
        .clone()
        .oneshot(authenticated_json_request(
            "/v1/chat/completions",
            json!({
                "model": "mock-fast-v1",
                "messages": [
                    {"role": "user", "content": "no stable prefix material"}
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(low_confidence_chat.status(), StatusCode::OK);

    let stats_response = app
        .clone()
        .oneshot(authenticated_get_request("/stats"))
        .await
        .unwrap();
    assert_eq!(stats_response.status(), StatusCode::OK);
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_text = String::from_utf8(stats_body.to_vec()).unwrap();
    assert!(!stats_text.contains(secret_prefix));
    let stats_json: Value = serde_json::from_str(&stats_text).unwrap();
    assert_eq!(stats_json["prefix_fingerprints_by_outcome"]["hit"], 2);
    assert_eq!(stats_json["prefix_fingerprints_by_outcome"]["miss"], 1);

    let metrics_response = app
        .oneshot(authenticated_get_request("/metrics"))
        .await
        .unwrap();
    assert_eq!(metrics_response.status(), StatusCode::OK);
    let metrics_body = to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_text = String::from_utf8(metrics_body.to_vec()).unwrap();
    assert!(!metrics_text.contains(secret_prefix));
    assert!(metrics_text.contains("rustygate_prefix_fingerprints_total{outcome=\"hit\"} 2\n"));
    assert!(metrics_text.contains("rustygate_prefix_fingerprints_total{outcome=\"miss\"} 1\n"));
}

#[tokio::test]
async fn stats_and_metrics_report_prefix_affinity_routing_reasons() {
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

    for suffix in ["A", "B"] {
        let response = app
            .clone()
            .oneshot(authenticated_json_request(
                "/v1/chat/completions",
                json!({
                    "model": "mock-fast",
                    "messages": [
                        {"role": "system", "content": "shared prefix for affinity stats"},
                        {"role": "user", "content": format!("volatile suffix {suffix}")}
                    ]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let stats_response = app
        .clone()
        .oneshot(authenticated_get_request("/stats"))
        .await
        .unwrap();
    let stats_body = to_bytes(stats_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: Value = serde_json::from_slice(&stats_body).unwrap();
    assert_eq!(
        stats_json["routing_decisions_by_policy_and_reason"]["prefix_affinity"]["prefix_miss"],
        1
    );
    assert_eq!(
        stats_json["routing_decisions_by_policy_and_reason"]["prefix_affinity"]["prefix_hit"],
        1
    );
    assert_eq!(
        stats_json["routing_decisions_by_policy_and_reason"]["prefix_affinity"]["selected"],
        2
    );

    let metrics_response = app
        .oneshot(authenticated_get_request("/metrics"))
        .await
        .unwrap();
    let metrics_body = to_bytes(metrics_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics_text = String::from_utf8(metrics_body.to_vec()).unwrap();
    assert!(metrics_text.contains(
        "rustygate_routing_decisions_total{policy=\"prefix_affinity\",reason=\"prefix_miss\"} 1\n"
    ));
    assert!(metrics_text.contains(
        "rustygate_routing_decisions_total{policy=\"prefix_affinity\",reason=\"prefix_hit\"} 1\n"
    ));
}

fn authenticated_json_request(route: &str, body: Value) -> Request<Body> {
    authenticated_json_post(route, body)
}

fn authenticated_get_request(route: &str) -> Request<Body> {
    authenticated_get(route)
}
