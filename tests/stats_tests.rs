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
    assert!(body.contains("rustygate_request_errors_total{category=\"provider_unavailable\"} 1\n"));
    assert!(body.contains(
        "rustygate_provider_errors_total{provider=\"mock-failing\",category=\"provider_unavailable\"} 1\n"
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
