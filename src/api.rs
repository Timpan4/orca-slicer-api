use crate::error::AppError;
use crate::slice::{AppState, progress, slice};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
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
        .route("/schema/{kind}", get(profile_schema))
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
        "calibration": {
            "available": true,
            "version": "1",
            "steps": ["temperature", "flow_rate", "pressure_advance", "retraction", "volumetric_flow"],
        },
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

async fn profile_schema(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<Json<Value>, AppError> {
    if !matches!(kind.as_str(), "printer" | "filament") {
        return Err(AppError::Bad("schema kind must be printer or filament".into()));
    }
    let path =
        state.config.schema_path.parent().ok_or(AppError::Internal)?.join(format!("{kind}.json"));
    let bytes = tokio::fs::read(path).await.map_err(|_| {
        AppError::Unavailable("profile schema is not packaged in this image".into())
    })?;
    let value: Value = serde_json::from_slice(&bytes)?;
    crate::contract::validate_bounds(&value).map_err(AppError::Bad)?;
    let schema: crate::contract::ProcessSchema = serde_json::from_value(value)?;
    let hash = crate::contract::sha256_json(&serde_json::to_value(&schema)?)
        .map_err(|_| AppError::Internal)?;
    let contract = &state.contract;
    Ok(Json(json!({
        "contract_version": contract.contract_version,
        "engine": contract.engine,
        "image_identity": contract.image_identity,
        "schema_hash": hash,
        "capabilities": contract.capabilities,
        "supported_scopes": ["global"],
        "pages": schema.pages, "options":schema.options,
        "scopes":schema.scopes, "samples":schema.samples,
    })))
}
