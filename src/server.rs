use std::{
    future::{pending, Future},
    time::Duration,
};

use anyhow::{anyhow, Result};
use tokio::time::timeout;
use tracing::{info, warn};

pub async fn run_with_graceful_shutdown<S, Shutdown, Trigger>(
    server: S,
    shutdown_signal: Shutdown,
    grace_period: Duration,
    trigger_shutdown: Trigger,
) -> Result<()>
where
    S: Future<Output = Result<()>>,
    Shutdown: Future<Output = ()>,
    Trigger: FnOnce(),
{
    tokio::pin!(server);
    tokio::pin!(shutdown_signal);

    tokio::select! {
        result = &mut server => result,
        _ = &mut shutdown_signal => {
            info!(grace_period_ms = grace_period.as_millis(), "shutdown signal received");
            trigger_shutdown();

            match timeout(grace_period, &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    warn!(grace_period_ms = grace_period.as_millis(), "shutdown grace period elapsed");
                    Err(anyhow!(
                        "shutdown grace period of {}ms elapsed before in-flight requests drained",
                        grace_period.as_millis()
                    ))
                }
            }
        }
    }
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(%error, "failed to install Ctrl-C shutdown handler");
            pending::<()>().await;
        }
    };

    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    warn!(%error, "failed to install SIGTERM shutdown handler");
                    pending::<()>().await;
                }
            }
        };

        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await;
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    use tokio::time::sleep;

    use super::run_with_graceful_shutdown;

    #[tokio::test]
    async fn graceful_shutdown_returns_when_server_finishes_before_signal() {
        let result = run_with_graceful_shutdown(
            async { Ok(()) },
            async {
                sleep(Duration::from_secs(10)).await;
            },
            Duration::from_millis(10),
            || {},
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn graceful_shutdown_triggers_drain_after_signal() {
        let shutdown_triggered = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown_triggered);
        let trigger_shutdown = Arc::clone(&shutdown_triggered);

        let result = run_with_graceful_shutdown(
            async move {
                while !server_shutdown.load(Ordering::SeqCst) {
                    sleep(Duration::from_millis(1)).await;
                }
                Ok(())
            },
            async {},
            Duration::from_millis(50),
            move || {
                trigger_shutdown.store(true, Ordering::SeqCst);
            },
        )
        .await;

        assert!(result.is_ok());
        assert!(shutdown_triggered.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn graceful_shutdown_errors_when_drain_timeout_elapses() {
        let result = run_with_graceful_shutdown(
            async {
                sleep(Duration::from_secs(10)).await;
                Ok(())
            },
            async {},
            Duration::from_millis(1),
            || {},
        )
        .await;

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("shutdown grace period"));
    }
}
