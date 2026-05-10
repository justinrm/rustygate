use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    middleware::{from_fn, from_fn_with_state},
    routing::{get, post},
    Router,
};

use crate::routes;

use super::{
    middleware::{
        auth_middleware, per_key_rate_limit_middleware, pre_auth_rate_limit_middleware,
        request_id_middleware,
    },
    state::AppState,
};

pub fn router() -> Router {
    router_with_state(AppState::default())
}

pub fn router_with_state(mut state: AppState) -> Router {
    if !state.rate_limit_backend_is_redis {
        state.rate_limit_backend = Arc::new(state.rate_limiter.clone());
    }
    let max_chat_body_bytes = state.max_chat_body_bytes;
    let mut protected_routes = Router::new()
        .route("/v1/responses", post(routes::compat::responses))
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route("/v1/models", get(routes::models::list_models))
        .route("/stats", get(routes::stats::stats))
        .route("/stats/providers", get(routes::stats::provider_stats))
        .route("/metrics", get(routes::stats::prometheus_metrics));

    if state.route_exposure.placeholder_compat_routes {
        protected_routes = protected_routes
            .route("/v1/embeddings", post(routes::compat::embeddings))
            .route("/v1/moderations", post(routes::compat::moderations))
            .route(
                "/v1/images/generations",
                post(routes::compat::image_generation),
            )
            .route("/v1/images/edits", post(routes::compat::image_edit))
            .route(
                "/v1/images/variations",
                post(routes::compat::image_variation),
            )
            .route(
                "/v1/audio/transcriptions",
                post(routes::compat::audio_transcription),
            )
            .route(
                "/v1/audio/translations",
                post(routes::compat::audio_translation),
            )
            .route(
                "/v1/files",
                get(routes::compat::list_files).post(routes::compat::create_file),
            )
            .route(
                "/v1/files/{file_id}",
                get(routes::compat::retrieve_file).delete(routes::compat::delete_file),
            )
            .route(
                "/v1/files/{file_id}/content",
                get(routes::compat::file_content),
            )
            .route(
                "/v1/batches",
                get(routes::compat::list_batches).post(routes::compat::create_batch),
            )
            .route(
                "/v1/batches/{batch_id}",
                get(routes::compat::retrieve_batch),
            )
            .route(
                "/v1/batches/{batch_id}/cancel",
                post(routes::compat::cancel_batch),
            )
            .route(
                "/v1/fine_tuning/jobs",
                get(routes::compat::list_fine_tuning_jobs)
                    .post(routes::compat::create_fine_tuning_job),
            )
            .route(
                "/v1/fine_tuning/jobs/{job_id}",
                get(routes::compat::retrieve_fine_tuning_job),
            )
            .route(
                "/v1/fine_tuning/jobs/{job_id}/cancel",
                post(routes::compat::cancel_fine_tuning_job),
            )
            .route(
                "/v1/fine_tuning/jobs/{job_id}/events",
                get(routes::compat::list_fine_tuning_events),
            )
            .route(
                "/v1/realtime/sessions",
                post(routes::compat::realtime_session),
            );
    }

    let protected_routes = protected_routes
        .layer(DefaultBodyLimit::max(max_chat_body_bytes))
        .layer(from_fn_with_state(
            state.clone(),
            per_key_rate_limit_middleware,
        ))
        .layer(from_fn_with_state(state.clone(), auth_middleware))
        .layer(from_fn_with_state(
            state.clone(),
            pre_auth_rate_limit_middleware,
        ))
        .layer(from_fn(request_id_middleware));

    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .merge(protected_routes)
        .with_state(state)
}
