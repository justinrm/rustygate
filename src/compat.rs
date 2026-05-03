use time::OffsetDateTime;
use uuid::Uuid;

/// Returns an OpenAI-style public identifier while keeping the UUID request ID
/// available internally for logs, metrics, and persisted metadata.
pub fn openai_id(prefix: &str, request_id: Uuid) -> String {
    format!("{prefix}-{}", request_id.simple())
}

pub fn unix_timestamp() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
