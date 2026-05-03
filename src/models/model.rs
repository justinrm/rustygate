use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ModelListResponse {
    pub object: &'static str,
    pub data: Vec<ModelResponse>,
}

#[derive(Debug, Serialize)]
pub struct ModelResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: &'static str,
    #[serde(skip_serializing)]
    pub resolved_model: String,
    #[serde(skip_serializing)]
    pub providers: Vec<String>,
}
