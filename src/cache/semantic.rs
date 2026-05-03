//! Optional semantic response cache.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::{
    cache::response::{CacheKey, ResponseCache},
    models::chat::ChatCompletionResponse,
    providers::provider::ProviderError,
};

#[derive(Debug, Clone)]
pub struct SemanticCachePolicy {
    pub similarity_threshold: f32,
}

impl Default for SemanticCachePolicy {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.95,
        }
    }
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, ProviderError>;
}

#[derive(Clone)]
pub struct SemanticResponseCache {
    inner: Arc<dyn ResponseCache>,
    embedder: Arc<dyn EmbeddingProvider>,
    entries: Arc<RwLock<Vec<SemanticEntry>>>,
    policy: SemanticCachePolicy,
    ttl: Duration,
}

#[derive(Debug, Clone)]
struct SemanticEntry {
    key: CacheKey,
    model: String,
    embedding: Vec<f32>,
}

impl SemanticResponseCache {
    pub fn new(
        inner: Arc<dyn ResponseCache>,
        embedder: Arc<dyn EmbeddingProvider>,
        policy: SemanticCachePolicy,
        ttl: Duration,
    ) -> Self {
        Self {
            inner,
            embedder,
            entries: Arc::new(RwLock::new(Vec::new())),
            policy,
            ttl,
        }
    }

    pub async fn get_semantic(
        &self,
        prompt_text: &str,
        model: &str,
    ) -> Result<Option<ChatCompletionResponse>, ProviderError> {
        let embedding = self.embedder.embed(prompt_text).await?;
        let entries = self.entries.read().await;
        let nearest = entries
            .iter()
            .filter(|entry| entry.model == model)
            .filter_map(|entry| {
                cosine_similarity(&embedding, &entry.embedding)
                    .map(|similarity| (entry, similarity))
            })
            .filter(|(_, similarity)| is_semantic_hit(&self.policy, *similarity))
            .max_by(|(_, left), (_, right)| left.total_cmp(right));

        let Some((entry, _)) = nearest else {
            return Ok(None);
        };
        Ok(self.inner.get(&entry.key).await)
    }

    pub async fn put_semantic(
        &self,
        key: CacheKey,
        prompt_text: &str,
        model: String,
        response: ChatCompletionResponse,
    ) -> Result<(), ProviderError> {
        let embedding = self.embedder.embed(prompt_text).await?;
        self.inner.put(key.clone(), response, self.ttl).await;
        self.entries.write().await.push(SemanticEntry {
            key,
            model,
            embedding,
        });
        Ok(())
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }

    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;

    for (left, right) in left.iter().zip(right.iter()) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }

    if left_norm == 0.0 || right_norm == 0.0 {
        return None;
    }

    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

pub fn is_semantic_hit(policy: &SemanticCachePolicy, similarity: f32) -> bool {
    similarity >= policy.similarity_threshold
}

#[cfg(test)]
mod tests {
    use super::{cosine_similarity, is_semantic_hit, SemanticCachePolicy};

    #[test]
    fn similar_vectors_cross_default_threshold() {
        let similarity = cosine_similarity(&[1.0, 0.0, 0.0], &[0.99, 0.01, 0.0]).unwrap();
        assert!(is_semantic_hit(&SemanticCachePolicy::default(), similarity));
    }
}
