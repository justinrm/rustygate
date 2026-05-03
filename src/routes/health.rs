use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use crate::{
    app::AppState,
    routing::health::{ProviderHealthSnapshot, ProviderHealthStatus},
};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReadyQuery {
    #[serde(default)]
    pub detail: bool,
}

#[derive(Debug, Serialize)]
pub struct ReadyResponse {
    pub status: &'static str,
    pub service: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, ProviderHealthResponse>>,
}

#[derive(Debug, Serialize)]
pub struct ProviderHealthResponse {
    pub status: &'static str,
    pub checked_at_unix_seconds: Option<u64>,
    pub error_category: Option<&'static str>,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "rustygate",
    })
}

pub async fn ready(
    State(state): State<AppState>,
    Query(query): Query<ReadyQuery>,
) -> (StatusCode, Json<ReadyResponse>) {
    let ready = state.provider_health.any_provider_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let providers = query.detail.then(|| {
        state
            .provider_health
            .snapshot()
            .into_iter()
            .map(|(name, snapshot)| (name, ProviderHealthResponse::from(snapshot)))
            .collect()
    });

    (
        status,
        Json(ReadyResponse {
            status: if ready { "ready" } else { "not_ready" },
            service: "rustygate",
            providers,
        }),
    )
}

impl From<ProviderHealthSnapshot> for ProviderHealthResponse {
    fn from(snapshot: ProviderHealthSnapshot) -> Self {
        let status = match snapshot.status {
            ProviderHealthStatus::Unknown => "unknown",
            ProviderHealthStatus::Healthy => "healthy",
            ProviderHealthStatus::Unhealthy => "unhealthy",
        };
        Self {
            status,
            checked_at_unix_seconds: snapshot.checked_at_unix_seconds,
            error_category: snapshot.error_category.map(|category| category.as_str()),
        }
    }
}
