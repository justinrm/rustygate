use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use moka::future::Cache;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::models::chat::{ChatCompletionRequest, ChatCompletionResponse};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(String);

impl CacheKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[async_trait]
pub trait ResponseCache: Send + Sync {
    async fn get(&self, key: &CacheKey) -> Option<ChatCompletionResponse>;
    async fn put(&self, key: CacheKey, response: ChatCompletionResponse, ttl: Duration);
}

#[derive(Clone)]
pub struct MemoryResponseCache {
    entries: Cache<CacheKey, ChatCompletionResponse>,
}

impl MemoryResponseCache {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Cache::builder()
                .max_capacity(max_entries.max(1) as u64)
                .time_to_live(ttl)
                .build(),
        }
    }
}

#[async_trait]
impl ResponseCache for MemoryResponseCache {
    async fn get(&self, key: &CacheKey) -> Option<ChatCompletionResponse> {
        self.entries.get(key).await
    }

    async fn put(&self, key: CacheKey, response: ChatCompletionResponse, _ttl: Duration) {
        self.entries.insert(key, response).await;
    }
}

#[derive(Clone)]
pub struct SqliteResponseCache {
    pool: SqlitePool,
}

impl SqliteResponseCache {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn prune_expired(&self) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM cache_entries WHERE expires_at_unix_seconds <= ?")
            .bind(unix_seconds() as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ResponseCache for SqliteResponseCache {
    async fn get(&self, key: &CacheKey) -> Option<ChatCompletionResponse> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT response_json FROM cache_entries WHERE cache_key = ? AND expires_at_unix_seconds > ?",
        )
        .bind(key.as_str())
        .bind(unix_seconds() as i64)
        .fetch_optional(&self.pool)
        .await
        .ok()?;
        row.and_then(|(json,)| serde_json::from_str(&json).ok())
    }

    async fn put(&self, key: CacheKey, response: ChatCompletionResponse, ttl: Duration) {
        let Ok(response_json) = serde_json::to_string(&response) else {
            return;
        };
        let expires_at = unix_seconds().saturating_add(ttl.as_secs()) as i64;
        let _ = sqlx::query(
            r#"
            INSERT INTO cache_entries (cache_key, response_json, expires_at_unix_seconds)
            VALUES (?, ?, ?)
            ON CONFLICT(cache_key) DO UPDATE SET
                response_json = excluded.response_json,
                expires_at_unix_seconds = excluded.expires_at_unix_seconds
            "#,
        )
        .bind(key.as_str())
        .bind(response_json)
        .bind(expires_at)
        .execute(&self.pool)
        .await;
    }
}

pub fn cache_key_for_request(request: &ChatCompletionRequest) -> Option<CacheKey> {
    if request.stream_enabled() || request.temperature.unwrap_or_default() > 0.0 {
        return None;
    }
    let canonical = CanonicalChatRequest {
        model: request.model.as_deref(),
        messages: &request.messages,
        tools: request.tools.as_ref(),
        tool_choice: request.tool_choice.as_ref(),
        response_format: request.response_format.as_ref(),
        max_tokens: request.max_tokens,
        parallel_tool_calls: request.parallel_tool_calls,
    };
    let encoded = serde_json::to_vec(&canonical).ok()?;
    let digest = Sha256::digest(encoded);
    let key = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(CacheKey(key))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[derive(Serialize)]
struct CanonicalChatRequest<'a> {
    model: Option<&'a str>,
    messages: &'a [crate::models::chat::ChatMessage],
    tools: Option<&'a Vec<crate::models::chat::Tool>>,
    tool_choice: Option<&'a crate::models::chat::ToolChoice>,
    response_format: Option<&'a serde_json::Value>,
    max_tokens: Option<u32>,
    parallel_tool_calls: Option<bool>,
}
