#![allow(dead_code)]

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Request},
};
use rustygate::providers::{
    mock::MockProvider,
    provider::{ChatProvider, ProviderEntry, ProviderPricing},
};
use serde_json::Value;

pub const TEST_GATEWAY_KEY: &str = "test-gateway-key";

pub fn authenticated_get(route: &str) -> Request<Body> {
    Request::builder()
        .uri(route)
        .method("GET")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_GATEWAY_KEY}"))
        .body(Body::empty())
        .unwrap()
}

pub fn authenticated_json_post(route: &str, body: Value) -> Request<Body> {
    authenticated_json_post_with_token(route, body, TEST_GATEWAY_KEY)
}

pub fn authenticated_json_post_with_token(route: &str, body: Value, token: &str) -> Request<Body> {
    Request::builder()
        .uri(route)
        .method("POST")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn json_post_without_auth(route: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(route)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn chat_request(body: Value) -> Request<Body> {
    authenticated_json_post("/v1/chat/completions", body)
}

pub fn chat_request_with_token(body: Value, token: &str) -> Request<Body> {
    authenticated_json_post_with_token("/v1/chat/completions", body, token)
}

pub fn responses_request(body: Value) -> Request<Body> {
    authenticated_json_post("/v1/responses", body)
}

pub fn mock_provider_entry(name: &str, model: &str, priority: u32) -> ProviderEntry {
    ProviderEntry {
        priority,
        provider: Arc::new(MockProvider::new(name, model)) as Arc<dyn ChatProvider>,
        pricing: ProviderPricing::default(),
    }
}
