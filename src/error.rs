use axum::{
    http::{
        header::{HeaderValue, RETRY_AFTER},
        StatusCode,
    },
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::providers::provider::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid request: {message}")]
    InvalidRequest {
        message: String,
        request_id: Option<Uuid>,
    },
    #[error("unauthorized")]
    Unauthorized {
        message: String,
        request_id: Option<Uuid>,
    },
    #[error("request rate limited")]
    GatewayRateLimited {
        request_id: Option<Uuid>,
        retry_after_seconds: u64,
    },
    #[error("no provider available")]
    NoProviderAvailable { request_id: Option<Uuid> },
    #[error("provider rate limited the request")]
    ProviderRateLimited { request_id: Option<Uuid> },
    #[error("provider timed out")]
    ProviderTimeout { request_id: Option<Uuid> },
    #[error("provider request failed")]
    ProviderFailure { request_id: Option<Uuid> },
    #[error("internal error")]
    Internal { request_id: Option<Uuid> },
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    request_id: Option<Uuid>,
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::InvalidRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
            Self::GatewayRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::NoProviderAvailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::ProviderRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::ProviderTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
            Self::ProviderFailure { .. } => StatusCode::BAD_GATEWAY,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "invalid_request",
            Self::Unauthorized { .. } => "unauthorized",
            Self::GatewayRateLimited { .. } => "gateway_rate_limited",
            Self::NoProviderAvailable { .. } => "no_provider_available",
            Self::ProviderRateLimited { .. } => "provider_rate_limited",
            Self::ProviderTimeout { .. } => "provider_timeout",
            Self::ProviderFailure { .. } => "provider_failure",
            Self::Internal { .. } => "internal",
        }
    }

    fn request_id(&self) -> Option<Uuid> {
        match self {
            Self::InvalidRequest { request_id, .. }
            | Self::Unauthorized { request_id, .. }
            | Self::GatewayRateLimited { request_id, .. }
            | Self::NoProviderAvailable { request_id }
            | Self::ProviderRateLimited { request_id }
            | Self::ProviderTimeout { request_id }
            | Self::ProviderFailure { request_id }
            | Self::Internal { request_id } => *request_id,
        }
    }

    pub fn public_message(&self) -> String {
        match self {
            Self::InvalidRequest { message, .. } => message.clone(),
            Self::Unauthorized { message, .. } => message.clone(),
            Self::GatewayRateLimited { .. } => "request rate limit exceeded, retry later".into(),
            Self::NoProviderAvailable { .. } => "no provider is available for this request".into(),
            Self::ProviderRateLimited { .. } => "provider rate limited this request".into(),
            Self::ProviderTimeout { .. } => "provider timed out while handling this request".into(),
            Self::ProviderFailure { .. } => "provider failed to process this request".into(),
            Self::Internal { .. } => "an internal error occurred".into(),
        }
    }

    pub fn from_provider_error(error: ProviderError, request_id: Option<Uuid>) -> Self {
        match error {
            ProviderError::RateLimited => Self::ProviderRateLimited { request_id },
            ProviderError::Timeout => Self::ProviderTimeout { request_id },
            ProviderError::AuthenticationFailed
            | ProviderError::ProviderUnavailable
            | ProviderError::ProviderBadResponse => Self::ProviderFailure { request_id },
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let retry_after = match &self {
            Self::GatewayRateLimited {
                retry_after_seconds,
                ..
            } => Some(*retry_after_seconds),
            _ => None,
        };
        let status = self.status_code();
        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code(),
                message: self.public_message(),
                request_id: self.request_id(),
            },
        };
        let mut response = (status, Json(body)).into_response();
        if let Some(retry_after_seconds) = retry_after {
            if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.max(1).to_string()) {
                response.headers_mut().insert(RETRY_AFTER, value);
            }
        }
        response
    }
}
