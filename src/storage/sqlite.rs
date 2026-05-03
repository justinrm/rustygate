//! Optional SQLite request-log storage.

use std::collections::BTreeMap;

use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use thiserror::Error;
use uuid::Uuid;

use crate::telemetry::{
    metrics::MetricsSnapshot,
    request_log::{RequestLogEntry, RequestLogStatus},
};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to connect to SQLite: {0}")]
    Connect(#[from] sqlx::Error),
    #[error("failed to run SQLite migrations: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("failed to serialize request log field: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SqliteRequestLogStore {
    pool: SqlitePool,
}

impl SqliteRequestLogStore {
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let options = database_url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record_request(&self, entry: &RequestLogEntry) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let prompt_messages_json = entry
            .prompt_messages
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let usage = entry.usage.as_ref();
        let cost = entry.cost_estimate.as_ref();

        sqlx::query(
            r#"
            INSERT INTO request_logs (
                id,
                route,
                created_at_unix_seconds,
                requested_model,
                final_provider,
                status,
                latency_ms,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                input_cost_usd,
                output_cost_usd,
                total_cost_usd,
                error_category,
                prompt_messages_json
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(entry.request_id.to_string())
        .bind(&entry.route)
        .bind(entry.created_at_unix_seconds)
        .bind(&entry.requested_model)
        .bind(&entry.final_provider)
        .bind(entry.status.as_str())
        .bind(entry.latency_ms as i64)
        .bind(usage.map(|usage| i64::from(usage.prompt_tokens)))
        .bind(usage.map(|usage| i64::from(usage.completion_tokens)))
        .bind(usage.map(|usage| i64::from(usage.total_tokens)))
        .bind(cost.map(|cost| cost.input_cost_usd))
        .bind(cost.map(|cost| cost.output_cost_usd))
        .bind(cost.map(|cost| cost.total_cost_usd))
        .bind(entry.error_category.map(|category| category.as_str()))
        .bind(prompt_messages_json)
        .execute(&mut *transaction)
        .await?;

        for attempt in &entry.provider_attempts {
            sqlx::query(
                r#"
                INSERT INTO provider_attempts (
                    id,
                    request_id,
                    provider_name,
                    attempt_order,
                    success,
                    is_fallback,
                    error_category,
                    latency_ms
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(entry.request_id.to_string())
            .bind(&attempt.provider_name)
            .bind(i64::from(attempt.attempt_order))
            .bind(if attempt.success { 1_i64 } else { 0_i64 })
            .bind(if attempt.is_fallback { 1_i64 } else { 0_i64 })
            .bind(attempt.error_category.map(|category| category.as_str()))
            .bind(attempt.latency_ms as i64)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(())
    }

    pub async fn stats_snapshot(&self) -> Result<MetricsSnapshot, StorageError> {
        let request_rows = sqlx::query(
            r#"
            SELECT
                status,
                latency_ms,
                COALESCE(prompt_tokens, 0) AS prompt_tokens,
                COALESCE(completion_tokens, 0) AS completion_tokens,
                COALESCE(total_tokens, 0) AS total_tokens,
                COALESCE(input_cost_usd, 0.0) AS input_cost_usd,
                COALESCE(output_cost_usd, 0.0) AS output_cost_usd,
                COALESCE(total_cost_usd, 0.0) AS total_cost_usd
            FROM request_logs
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        let attempt_rows = sqlx::query(
            r#"
            SELECT provider_name, success, is_fallback, latency_ms
            FROM provider_attempts
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut snapshot = MetricsSnapshot::default();
        let mut request_latencies = Vec::new();
        let mut provider_latencies: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut provider_total_latency_ms: BTreeMap<String, u128> = BTreeMap::new();

        for row in request_rows {
            snapshot.total_requests += 1;
            match row.get::<String, _>("status").as_str() {
                "success" => snapshot.successful_requests += 1,
                _ => snapshot.failed_requests += 1,
            }

            let latency_ms = row.get::<i64, _>("latency_ms") as u64;
            request_latencies.push(latency_ms);
            snapshot.avg_latency_ms += latency_ms as f64;
            snapshot.estimated_prompt_tokens += row.get::<i64, _>("prompt_tokens") as u64;
            snapshot.estimated_completion_tokens += row.get::<i64, _>("completion_tokens") as u64;
            snapshot.estimated_total_tokens += row.get::<i64, _>("total_tokens") as u64;
            snapshot.estimated_input_cost_usd += row.get::<f64, _>("input_cost_usd");
            snapshot.estimated_output_cost_usd += row.get::<f64, _>("output_cost_usd");
            snapshot.estimated_total_cost_usd += row.get::<f64, _>("total_cost_usd");
        }

        if snapshot.total_requests > 0 {
            snapshot.avg_latency_ms /= snapshot.total_requests as f64;
            snapshot.error_rate = snapshot.failed_requests as f64 / snapshot.total_requests as f64;
        }
        snapshot.p95_latency_ms = percentile_latency(&request_latencies, 0.95);

        for row in attempt_rows {
            snapshot.total_provider_attempts += 1;
            let provider_name = row.get::<String, _>("provider_name");
            let success = row.get::<i64, _>("success") == 1;
            let is_fallback = row.get::<i64, _>("is_fallback") == 1;
            let latency_ms = row.get::<i64, _>("latency_ms") as u64;

            *snapshot
                .requests_by_provider
                .entry(provider_name.clone())
                .or_default() += 1;
            *provider_total_latency_ms
                .entry(provider_name.clone())
                .or_default() += u128::from(latency_ms);
            provider_latencies
                .entry(provider_name.clone())
                .or_default()
                .push(latency_ms);

            if success {
                *snapshot
                    .successes_by_provider
                    .entry(provider_name.clone())
                    .or_default() += 1;
            } else {
                *snapshot
                    .errors_by_provider
                    .entry(provider_name.clone())
                    .or_default() += 1;
            }

            if is_fallback {
                snapshot.fallback_attempts += 1;
                *snapshot
                    .fallback_attempts_by_provider
                    .entry(provider_name)
                    .or_default() += 1;
            }
        }

        for (provider_name, requests) in &snapshot.requests_by_provider {
            let total_latency_ms = provider_total_latency_ms
                .get(provider_name)
                .copied()
                .unwrap_or_default();
            snapshot.avg_latency_ms_by_provider.insert(
                provider_name.clone(),
                total_latency_ms as f64 / *requests as f64,
            );
        }

        for (provider_name, samples) in provider_latencies {
            snapshot
                .p95_latency_ms_by_provider
                .insert(provider_name, percentile_latency(&samples, 0.95));
        }

        Ok(snapshot)
    }

    pub async fn count_request_logs(&self) -> Result<i64, StorageError> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM request_logs")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get("count"))
    }

    pub async fn count_provider_attempts(&self) -> Result<i64, StorageError> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM provider_attempts")
            .fetch_one(&self.pool)
            .await?;

        Ok(row.get("count"))
    }

    pub async fn count_logs_with_prompt_content(&self) -> Result<i64, StorageError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS count FROM request_logs WHERE prompt_messages_json IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get("count"))
    }
}

fn percentile_latency(samples: &[u64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index] as f64
}

#[allow(dead_code)]
fn _status_is_success(status: RequestLogStatus) -> bool {
    matches!(status, RequestLogStatus::Success)
}
