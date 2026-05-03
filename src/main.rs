use std::{net::SocketAddr, time::Duration};

use rustygate::{app, config::AppConfig, server, telemetry};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let config = AppConfig::from_env()?;
    telemetry::tracing::init_tracing(&config.telemetry)?;
    let addr = SocketAddr::new(config.server.host, config.server.port);
    let shutdown_grace_period = Duration::from_millis(config.server.shutdown_grace_period_ms);
    let request_logging_enabled = config.gateway.enable_request_logging;
    let prompt_logging_enabled = config.gateway.log_prompt_content;
    let storage_enabled = config.storage.enabled;
    let state = app::AppState::from_config(&config).await?;
    rustygate::routing::health::spawn_provider_health_probes(
        state.clone(),
        Duration::from_millis(config.gateway.health_check_interval_ms),
    );
    if let Some(store) = state.request_log_store.clone() {
        let retention_days = config.storage.retention_days;
        tokio::spawn(async move {
            let interval = Duration::from_secs(86_400);
            loop {
                let cutoff =
                    current_unix_seconds().saturating_sub(retention_days.saturating_mul(86_400));
                if let Err(error) = store.prune_older_than(cutoff).await {
                    warn!(error = %error, "failed to prune retained SQLite telemetry rows");
                }
                tokio::time::sleep(interval).await;
            }
        });
    }
    let provider_names = state.provider_names();
    let provider_count = provider_names.len();
    let app = app::router_with_state(state).layer(TraceLayer::new_for_http());

    if prompt_logging_enabled {
        warn!("prompt content logging is enabled for local development");
    }

    info!(
        %addr,
        provider_count,
        providers = ?provider_names,
        request_logging_enabled,
        prompt_logging_enabled,
        storage_enabled,
        shutdown_grace_period_ms = shutdown_grace_period.as_millis(),
        "starting RustyGate"
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(anyhow::Error::from)
    };

    server::run_with_graceful_shutdown(
        server,
        server::shutdown_signal(),
        shutdown_grace_period,
        move || {
            let _ = shutdown_tx.send(());
        },
    )
    .await?;

    Ok(())
}

fn current_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
