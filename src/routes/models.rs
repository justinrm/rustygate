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

    let mut models = Vec::new();
    for (model, providers) in &providers_by_model {
        models.push(ModelResponse {
            id: model.clone(),
            object: "model",
            created: MODEL_CREATED_AT,
            owned_by: "rustygate",
            resolved_model: model.clone(),
            providers: providers.iter().cloned().collect(),
        });
    }

    for (alias, resolved_model) in state.model_aliases.iter() {
        let Some(providers) = providers_by_model.get(resolved_model) else {
            continue;
        };
        if providers_by_model.contains_key(alias) {
            continue;
        }

        models.push(ModelResponse {
            id: alias.clone(),
            object: "model",
            created: MODEL_CREATED_AT,
            owned_by: "rustygate",
            resolved_model: resolved_model.clone(),
            providers: providers.iter().cloned().collect(),
        });
    }

    models.sort_by(|left, right| left.id.cmp(&right.id));

    Json(ModelListResponse {
        object: "list",
        data: models,
    })
}
