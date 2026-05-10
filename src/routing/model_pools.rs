use std::collections::{BTreeMap, BTreeSet};

use crate::config::{ModelPoolConfig, RoutingPolicy};

#[derive(Debug, Clone, Default)]
pub struct ModelPool {
    pub name: String,
    pub public_model_ids: Vec<String>,
    pub routing_policy: Option<RoutingPolicy>,
    pub members: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelPoolIndex {
    pools_by_name: BTreeMap<String, ModelPool>,
    pool_by_public_model_id: BTreeMap<String, String>,
}

impl ModelPoolIndex {
    pub fn from_configs(configs: &[ModelPoolConfig]) -> Self {
        let mut pools_by_name = BTreeMap::new();
        let mut pool_by_public_model_id = BTreeMap::new();

        for config in configs {
            let mut public_model_ids = Vec::with_capacity(1 + config.aliases.len());
            public_model_ids.push(config.name.clone());
            for alias in &config.aliases {
                if alias != &config.name {
                    public_model_ids.push(alias.clone());
                }
            }
            public_model_ids.sort();
            public_model_ids.dedup();

            let members = config.members.iter().cloned().collect::<BTreeSet<_>>();
            let pool = ModelPool {
                name: config.name.clone(),
                public_model_ids: public_model_ids.clone(),
                routing_policy: config.routing_policy,
                members,
            };

            for public_model_id in public_model_ids {
                pool_by_public_model_id.insert(public_model_id, config.name.clone());
            }
            pools_by_name.insert(config.name.clone(), pool);
        }

        Self {
            pools_by_name,
            pool_by_public_model_id,
        }
    }

    pub fn pool_for_public_model(&self, model: &str) -> Option<&ModelPool> {
        let pool_name = self.pool_by_public_model_id.get(model)?;
        self.pools_by_name.get(pool_name)
    }

    pub fn members_for_public_model(&self, model: &str) -> Option<&BTreeSet<String>> {
        self.pool_for_public_model(model).map(|pool| &pool.members)
    }

    pub fn providers_for_model_id(&self, model: &str) -> Option<Vec<String>> {
        let mut providers = self
            .members_for_public_model(model)?
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        providers.sort();
        Some(providers)
    }

    pub fn public_model_ids(&self) -> Vec<String> {
        self.pool_by_public_model_id
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    }

    pub fn is_provider_in_any_pool(&self, provider_name: &str) -> bool {
        self.pools_by_name
            .values()
            .any(|pool| pool.members.contains(provider_name))
    }

    pub fn is_empty(&self) -> bool {
        self.pools_by_name.is_empty()
    }
}
