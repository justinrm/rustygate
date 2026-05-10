use std::collections::{BTreeMap, BTreeSet};

use axum::{extract::State, Json};

use crate::{
    app::AppState,
    models::model::{ModelListResponse, ModelResponse},
};

const MODEL_CREATED_AT: i64 = 1_700_000_000;

pub async fn list_models(State(state): State<AppState>) -> Json<ModelListResponse> {
    let mut providers_by_model: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in &state.providers {
        providers_by_model
            .entry(entry.provider.model().to_string())
            .or_default()
            .insert(entry.provider.name().to_string());
    }

    let pool_public_ids = state.model_pools.public_model_ids();
    let pool_public_id_set = pool_public_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut direct_models = Vec::new();
    for (model, providers) in &providers_by_model {
        let provider_names = providers.iter().map(String::as_str).collect::<Vec<_>>();
        let all_providers_in_pools = !provider_names.is_empty()
            && provider_names
                .iter()
                .all(|provider| state.model_pools.is_provider_in_any_pool(provider));
        let should_hide_internal_model = all_providers_in_pools
            && !pool_public_id_set.contains(model.as_str())
            && !state.model_aliases.contains_key(model);
        if should_hide_internal_model {
            continue;
        }
        direct_models.push((model.clone(), providers.clone()));
    }

    let mut models = Vec::new();
    let mut seen_ids = BTreeSet::new();
    for (model, providers) in direct_models {
        seen_ids.insert(model.clone());
        models.push(ModelResponse {
            id: model.clone(),
            object: "model",
            created: MODEL_CREATED_AT,
            owned_by: "rustygate",
            resolved_model: model,
            providers: providers.iter().cloned().collect(),
        });
    }

    for pool_model_id in pool_public_ids {
        let Some(pool_providers) = state.model_pools.providers_for_model_id(&pool_model_id) else {
            continue;
        };
        if !seen_ids.insert(pool_model_id.clone()) {
            continue;
        }

        models.push(ModelResponse {
            id: pool_model_id.clone(),
            object: "model",
            created: MODEL_CREATED_AT,
            owned_by: "rustygate",
            resolved_model: pool_model_id,
            providers: pool_providers,
        });
    }

    for (alias, resolved_model) in state.model_aliases.iter() {
        if !seen_ids.insert(alias.clone()) {
            continue;
        }

        let providers = if let Some(direct) = providers_by_model.get(resolved_model) {
            direct.iter().cloned().collect::<Vec<_>>()
        } else if let Some(pool_members) = state.model_pools.providers_for_model_id(resolved_model)
        {
            pool_members
        } else {
            continue;
        };

        models.push(ModelResponse {
            id: alias.clone(),
            object: "model",
            created: MODEL_CREATED_AT,
            owned_by: "rustygate",
            resolved_model: resolved_model.clone(),
            providers,
        });
    }

    models.sort_by(|left, right| left.id.cmp(&right.id));

    Json(ModelListResponse {
        object: "list",
        data: models,
    })
}
