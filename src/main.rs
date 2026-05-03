use std::net::SocketAddr;

use rustygate::{app, config::AppConfig};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = AppConfig::from_env()?;
    let addr = SocketAddr::new(config.server.host, config.server.port);
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
        "starting RustyGate"
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
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

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
