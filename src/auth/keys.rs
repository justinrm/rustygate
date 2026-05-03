use std::{
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use async_trait::async_trait;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    Admin,
    Inference,
    Observability,
}

impl KeyRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Inference => "inference",
            Self::Observability => "observability",
        }
    }

    pub fn allows_inference(self) -> bool {
        matches!(self, Self::Admin | Self::Inference)
    }

    pub fn allows_observability(self) -> bool {
        matches!(self, Self::Admin | Self::Observability)
    }
}

impl FromStr for KeyRole {
    type Err = AuthError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "admin" => Ok(Self::Admin),
            "inference" => Ok(Self::Inference),
            "observability" => Ok(Self::Observability),
            _ => Err(AuthError::InvalidRole(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyLimits {
    pub requests_per_minute: Option<u32>,
    pub daily_token_quota: Option<u64>,
    pub daily_cost_quota_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedKey {
    pub id: String,
    pub prefix: String,
    pub label: String,
    pub role: KeyRole,
    pub limits: KeyLimits,
    pub cache_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct GeneratedApiKey {
    pub id: String,
    pub prefix: String,
    pub raw_key: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid key role `{0}`")]
    InvalidRole(String),
    #[error("invalid API key format")]
    InvalidKeyFormat,
    #[error("failed to hash or verify API key")]
    Hash,
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

#[derive(Debug, Clone, Copy)]
pub struct QuotaRejection {
    pub retry_after_seconds: u64,
}

#[async_trait]
pub trait KeyStore: Send + Sync {
    async fn authenticate(&self, raw_key: &str) -> Result<Option<AuthenticatedKey>, AuthError>;

    async fn check_quota(&self, _key: &AuthenticatedKey) -> Result<(), QuotaRejection> {
        Ok(())
    }

    async fn record_usage(
        &self,
        _api_key_id: &str,
        _total_tokens: u64,
        _total_cost_usd: f64,
    ) -> Result<(), AuthError> {
        Ok(())
    }
}

pub type SharedKeyStore = Arc<dyn KeyStore>;

#[derive(Debug)]
pub struct StaticKeyStore {
    raw_key: String,
}

impl StaticKeyStore {
    pub fn new(raw_key: impl Into<String>) -> Self {
        Self {
            raw_key: raw_key.into(),
        }
    }
}

#[async_trait]
impl KeyStore for StaticKeyStore {
    async fn authenticate(&self, raw_key: &str) -> Result<Option<AuthenticatedKey>, AuthError> {
        if raw_key.as_bytes().ct_eq(self.raw_key.as_bytes()).into() {
            Ok(Some(AuthenticatedKey {
                id: "env-default".into(),
                prefix: "env".into(),
                label: "environment default key".into(),
                role: KeyRole::Admin,
                limits: KeyLimits::default(),
                cache_enabled: true,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteKeyStore {
    pool: SqlitePool,
}

impl SqliteKeyStore {
    pub async fn connect(database_url: &str) -> Result<Self, AuthError> {
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

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn create_key(
        &self,
        label: &str,
        role: KeyRole,
        limits: KeyLimits,
        cache_enabled: bool,
    ) -> Result<GeneratedApiKey, AuthError> {
        let id = Uuid::new_v4().to_string();
        let prefix = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>();
        let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let raw_key = format!("rgk_{prefix}_{secret}");
        let key_hash = hash_key(&raw_key)?;

        sqlx::query(
            r#"
            INSERT INTO api_keys (
                id, key_prefix, key_hash, label, role, requests_per_minute,
                daily_token_quota, daily_cost_quota_usd, cache_enabled,
                created_at_unix_seconds, revoked_at_unix_seconds
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
            "#,
        )
        .bind(&id)
        .bind(&prefix)
        .bind(key_hash)
        .bind(label)
        .bind(role.as_str())
        .bind(limits.requests_per_minute.map(i64::from))
        .bind(limits.daily_token_quota.map(|value| value as i64))
        .bind(limits.daily_cost_quota_usd)
        .bind(if cache_enabled { 1_i64 } else { 0_i64 })
        .bind(unix_seconds() as i64)
        .execute(&self.pool)
        .await?;

        Ok(GeneratedApiKey {
            id,
            prefix,
            raw_key,
        })
    }

    pub async fn list_keys(&self) -> Result<Vec<AuthenticatedKey>, AuthError> {
        let rows = sqlx::query(
            r#"
            SELECT id, key_prefix, label, role, requests_per_minute,
                   daily_token_quota, daily_cost_quota_usd, cache_enabled
            FROM api_keys
            WHERE revoked_at_unix_seconds IS NULL
            ORDER BY created_at_unix_seconds DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_key).collect()
    }

    pub async fn get_key(&self, id: &str) -> Result<Option<AuthenticatedKey>, AuthError> {
        let row = sqlx::query(
            r#"
            SELECT id, key_prefix, label, role, requests_per_minute,
                   daily_token_quota, daily_cost_quota_usd, cache_enabled
            FROM api_keys
            WHERE id = ? AND revoked_at_unix_seconds IS NULL
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_key).transpose()
    }

    pub async fn revoke_key(&self, id: &str) -> Result<(), AuthError> {
        sqlx::query("UPDATE api_keys SET revoked_at_unix_seconds = ? WHERE id = ?")
            .bind(unix_seconds() as i64)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl KeyStore for SqliteKeyStore {
    async fn authenticate(&self, raw_key: &str) -> Result<Option<AuthenticatedKey>, AuthError> {
        let prefix = parse_key_prefix(raw_key)?;
        let Some(row) = sqlx::query(
            r#"
            SELECT id, key_prefix, key_hash, label, role, requests_per_minute,
                   daily_token_quota, daily_cost_quota_usd, cache_enabled
            FROM api_keys
            WHERE key_prefix = ? AND revoked_at_unix_seconds IS NULL
            "#,
        )
        .bind(prefix)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let key_hash: String = row.try_get("key_hash")?;
        if !verify_key(raw_key, &key_hash)? {
            return Ok(None);
        }

        row_to_key(row).map(Some)
    }

    async fn check_quota(&self, key: &AuthenticatedKey) -> Result<(), QuotaRejection> {
        if key.limits.daily_token_quota.is_none() && key.limits.daily_cost_quota_usd.is_none() {
            return Ok(());
        }

        let day = current_day_unix();
        let row = sqlx::query(
            r#"
            SELECT total_tokens, total_cost_usd
            FROM api_key_usage_daily
            WHERE api_key_id = ? AND day_unix = ?
            "#,
        )
        .bind(&key.id)
        .bind(day as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| QuotaRejection {
            retry_after_seconds: seconds_until_next_day(),
        })?;

        let total_tokens = row
            .as_ref()
            .and_then(|row| row.try_get::<i64, _>("total_tokens").ok())
            .unwrap_or_default() as u64;
        let total_cost_usd = row
            .as_ref()
            .and_then(|row| row.try_get::<f64, _>("total_cost_usd").ok())
            .unwrap_or_default();

        if key
            .limits
            .daily_token_quota
            .is_some_and(|quota| total_tokens >= quota)
            || key
                .limits
                .daily_cost_quota_usd
                .is_some_and(|quota| total_cost_usd >= quota)
        {
            Err(QuotaRejection {
                retry_after_seconds: seconds_until_next_day(),
            })
        } else {
            Ok(())
        }
    }

    async fn record_usage(
        &self,
        api_key_id: &str,
        total_tokens: u64,
        total_cost_usd: f64,
    ) -> Result<(), AuthError> {
        sqlx::query(
            r#"
            INSERT INTO api_key_usage_daily (
                api_key_id, day_unix, request_count, total_tokens, total_cost_usd
            )
            VALUES (?, ?, 1, ?, ?)
            ON CONFLICT(api_key_id, day_unix) DO UPDATE SET
                request_count = request_count + 1,
                total_tokens = total_tokens + excluded.total_tokens,
                total_cost_usd = total_cost_usd + excluded.total_cost_usd
            "#,
        )
        .bind(api_key_id)
        .bind(current_day_unix() as i64)
        .bind(i64::try_from(total_tokens).unwrap_or(i64::MAX))
        .bind(total_cost_usd)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_key(row: sqlx::sqlite::SqliteRow) -> Result<AuthenticatedKey, AuthError> {
    let role: String = row.try_get("role")?;
    Ok(AuthenticatedKey {
        id: row.try_get("id")?,
        prefix: row.try_get("key_prefix")?,
        label: row.try_get("label")?,
        role: KeyRole::from_str(&role)?,
        limits: KeyLimits {
            requests_per_minute: row
                .try_get::<Option<i64>, _>("requests_per_minute")?
                .map(|value| value as u32),
            daily_token_quota: row
                .try_get::<Option<i64>, _>("daily_token_quota")?
                .map(|value| value as u64),
            daily_cost_quota_usd: row.try_get("daily_cost_quota_usd")?,
        },
        cache_enabled: row.try_get::<i64, _>("cache_enabled")? != 0,
    })
}

pub fn parse_key_prefix(raw_key: &str) -> Result<&str, AuthError> {
    let mut parts = raw_key.splitn(3, '_');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("rgk"), Some(prefix), Some(secret)) if !prefix.is_empty() && !secret.is_empty() => {
            Ok(prefix)
        }
        _ => Err(AuthError::InvalidKeyFormat),
    }
}

pub fn hash_key(raw_key: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(raw_key.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::Hash)
}

pub fn verify_key(raw_key: &str, hash: &str) -> Result<bool, AuthError> {
    let parsed_hash = PasswordHash::new(hash).map_err(|_| AuthError::Hash)?;
    Ok(Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed_hash)
        .is_ok())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn current_day_unix() -> u64 {
    unix_seconds() / 86_400
}

fn seconds_until_next_day() -> u64 {
    86_400 - (unix_seconds() % 86_400)
}
