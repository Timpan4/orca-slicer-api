use crate::slice::{AppState, progress, slice};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    routing::{get, post},
};
use serde_json::{Value, json};

const MAX_MULTIPART_BYTES: usize = 640 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/source", get(source))
        .route("/capabilities", get(capabilities))
        .route("/schema/process", get(schema))
        .route("/profiles/bundled", get(profiles))
        .route("/slice", post(slice).layer(DefaultBodyLimit::max(MAX_MULTIPART_BYTES)))
        .route("/slice/progress/{request_id}", get(progress))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let contract = &state.contract;
    Json(json!({
        "status": "ok",
        "contract_version": contract.contract_version,
        "engine": contract.engine,
        "image_identity": contract.image_identity,
        "schema_hash": contract.schema_hash,
    }))
}

async fn source() -> Json<Value> {
    Json(json!({
        "license": "AGPL-3.0-or-later",
        "url": "https://github.com/Timpan4/orca-slicer-api",
    }))
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    let contract = &state.contract;
    Json(json!({
        "contract_version": contract.contract_version,
        "engine": contract.engine,
        "image_identity": contract.image_identity,
        "schema_hash": contract.schema_hash,
        "capabilities": contract.capabilities,
        "supported_scopes": contract.supported_scopes,
    }))
}

async fn schema(State(state): State<AppState>) -> Json<Value> {
    let contract = &state.contract;
    let process = &contract.process_schema;
    Json(json!({
        "contract_version": contract.contract_version,
        "engine": contract.engine,
        "image_identity": contract.image_identity,
        "schema_hash": contract.schema_hash,
        "capabilities": contract.capabilities,
        "supported_scopes": contract.supported_scopes,
        "pages": process.pages,
        "options": process.options,
        "scopes": process.scopes,
        "samples": process.samples,
    }))
}

async fn profiles(State(state): State<AppState>) -> Json<Value> {
    Json((*state.profiles).clone())
}
