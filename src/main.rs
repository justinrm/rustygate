use std::{net::SocketAddr, time::Duration};

use rustygate::{app, config::AppConfig, server};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = AppConfig::from_env()?;
    let addr = SocketAddr::new(config.server.host, config.server.port);
    let shutdown_grace_period = Duration::from_millis(config.server.shutdown_grace_period_ms);
    let request_logging_enabled = config.gateway.enable_request_logging;
    let prompt_logging_enabled = config.gateway.log_prompt_content;
    let storage_enabled = config.storage.enabled;
    let state = app::AppState::from_config(&config).await?;
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rustygate=info,tower_http=info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
