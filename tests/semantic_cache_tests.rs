#![cfg(feature = "semantic-cache")]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use rustygate::{
    cache::{
        response::{cache_key_for_request, MemoryResponseCache},
        semantic::{EmbeddingProvider, SemanticCachePolicy, SemanticResponseCache},
    },
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    providers::provider::ProviderError,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn semantic_cache_hits_similar_prompt_and_misses_dissimilar_prompt() {
    let inner = Arc::new(MemoryResponseCache::new(Duration::from_secs(60), 100));
    let cache = SemanticResponseCache::new(
        inner,
        Arc::new(MockEmbedder),
        SemanticCachePolicy {
            similarity_threshold: 0.99,
        },
        Duration::from_secs(60),
    );
    let request: ChatCompletionRequest = serde_json::from_value(json!({
        "model": "mock-fast-v1",
        "messages": [{"role": "user", "content": "abc"}]
    }))
    .unwrap();
    let key = cache_key_for_request(&request).unwrap();
    let response = ChatCompletionResponse::placeholder(
        Uuid::new_v4(),
        "mock-fast-v1".into(),
        "mock-provider".into(),
    );

    cache
        .put_semantic(key, "abc", "mock-fast-v1".into(), response)
        .await
        .unwrap();

    assert!(cache
        .get_semantic("abd", "mock-fast-v1")
        .await
        .unwrap()
        .is_some());
    assert!(cache
        .get_semantic("zzzzzzzz", "mock-fast-v1")
        .await
        .unwrap()
        .is_none());
}

struct MockEmbedder;

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        let first = text.bytes().next().unwrap_or_default();
        if first == b'z' {
            Ok(vec![0.0, 1.0])
        } else {
            Ok(vec![1.0, 0.0])
        }
    }
}
