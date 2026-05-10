use axum::{
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::{auth::keys::AuthenticatedKey, error::AppError};

use super::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub Uuid);

pub(super) async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4);

    request.extensions_mut().insert(RequestId(request_id));
    next.run(request).await
}

pub(super) async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = request_id_from_extensions(&request);
    let Some(api_key) = bearer_token(&request) else {
        return AppError::Unauthorized {
            message: "missing bearer token".into(),
            request_id: Some(request_id),
        }
        .into_response();
    };
    let authenticated_key = match state.key_store.authenticate(api_key).await {
        Ok(Some(key)) => key,
        Ok(None) | Err(_) => {
            return AppError::Unauthorized {
                message: "invalid bearer token".into(),
                request_id: Some(request_id),
            }
            .into_response();
        }
    };

    if !role_allows_path(authenticated_key.role, request.uri().path()) {
        return AppError::Forbidden {
            message: "API key is not allowed to access this route".into(),
            request_id: Some(request_id),
        }
        .into_response();
    }

    request.extensions_mut().insert(authenticated_key);
    next.run(request).await
}

pub(super) async fn pre_auth_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = request_id_from_extensions(&request);
    if let Err(retry_after_seconds) = state.rate_limit_backend.check_global().await {
        return AppError::GatewayRateLimited {
            request_id: Some(request_id),
            retry_after_seconds,
        }
        .into_response();
    }

    next.run(request).await
}

pub(super) async fn per_key_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = request_id_from_extensions(&request);
    let Some(api_key) = request.extensions().get::<AuthenticatedKey>().cloned() else {
        return AppError::Unauthorized {
            message: "missing authenticated API key".into(),
            request_id: Some(request_id),
        }
        .into_response();
    };

    if let Err(quota) = state.key_store.check_quota(&api_key).await {
        return AppError::GatewayRateLimited {
            request_id: Some(request_id),
            retry_after_seconds: quota.retry_after_seconds,
        }
        .into_response();
    }

    let burst = api_key
        .limits
        .requests_per_minute
        .map(|rpm| (rpm / 4).max(1));
    if let Err(retry_after_seconds) = state
        .rate_limit_backend
        .check_key(&api_key.id, api_key.limits.requests_per_minute, burst)
        .await
    {
        return AppError::GatewayRateLimited {
            request_id: Some(request_id),
            retry_after_seconds,
        }
        .into_response();
    }

    next.run(request).await
}

fn bearer_token(request: &Request) -> Option<&str> {
    let header = request.headers().get(AUTHORIZATION)?;
    let header = header.to_str().ok()?;
    let token = header.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }
    Some(token)
}

fn request_id_from_extensions(request: &Request) -> Uuid {
    request
        .extensions()
        .get::<RequestId>()
        .map(|request_id| request_id.0)
        .unwrap_or_else(Uuid::new_v4)
}

fn role_allows_path(role: crate::auth::keys::KeyRole, path: &str) -> bool {
    if path == "/stats" || path == "/stats/providers" || path == "/metrics" {
        role.allows_observability()
    } else {
        role.allows_inference() || path == "/v1/models"
    }
}
