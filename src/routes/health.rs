use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "rustygate",
    })
}

pub async fn ready() -> Json<HealthResponse> {
    // Readiness is intentionally lightweight until provider health checks exist.
    Json(HealthResponse {
        status: "ready",
        service: "rustygate",
    })
}
