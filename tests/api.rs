use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use orca_slicer_api::{
    api,
    config::Config,
    contract::{
        Capabilities, Contract, Engine, Group, ImageIdentity, Page, ProcessSchema, ScopeValue,
        process_hash,
    },
    progress::ProgressStore,
    slice::AppState,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tower::ServiceExt;

fn contract() -> Contract {
    let mut contract = Contract {
        contract_version: "1".into(),
        engine: Engine {
            name: "OrcaSlicer".into(),
            version: "2.4.2".into(),
            commit: "8500fcdccaa10b5099ac20d252af3a7c560046f1".into(),
        },
        image_identity: ImageIdentity { digest: format!("sha256:{}", "a".repeat(64)) },
        schema_hash: String::new(),
        capabilities: Capabilities {
            process_schema: true,
            model_state: false,
            progress: true,
            cancel: false,
        },
        supported_scopes: vec!["global".into(), "object".into()],
        process_schema: ProcessSchema {
            pages: vec![Page {
                name: "Quality".into(),
                groups: vec![Group { name: "Layers".into(), options: vec!["layer_height".into()] }],
            }],
            options: vec![json!({"key":"layer_height","type":"number","min":0.04,"max":1.0})],
            scopes: BTreeMap::from([(
                "layer_height".into(),
                ScopeValue::Many(vec!["global".into(), "object".into()]),
            )]),
            samples: BTreeMap::from([("layer_height".into(), json!(0.2))]),
        },
    };
    contract.schema_hash = process_hash(&contract).unwrap();
    contract
}

fn state() -> AppState {
    let contract = contract();
    AppState {
        config: Config {
            data_dir: PathBuf::from("/tmp/orca-api-test"),
            cli_path: "orca-slicer".into(),
            bridge_path: None,
            image_digest: contract.image_identity.digest.clone(),
            schema_path: PathBuf::from("unused"),
            profiles_path: None,
            profile_source_path: None,
            max_concurrency: 1,
        },
        contract: Arc::new(contract),
        profiles: Arc::new(json!({"printer":[],"process":[],"filament":[]})),
        profile_catalog: Arc::new(Default::default()),
        progress: ProgressStore::new(),
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

async fn json_response(app: axum::Router, path: &str) -> (StatusCode, Value) {
    let response = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn discovery_contract_matches_layercove_shape() {
    let app = api::router(state());
    let (status, capabilities) = json_response(app.clone(), "/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["contract_version"], "1");
    assert_eq!(capabilities["engine"]["name"], "OrcaSlicer");
    assert_eq!(capabilities["capabilities"]["cancel"], false);
    assert!(capabilities.get("process_schema").is_none());

    let (status, schema) = json_response(app, "/schema/process").await;
    assert_eq!(status, StatusCode::OK);
    assert!(schema["pages"].is_array());
    assert!(schema["options"].is_array());
    assert_eq!(schema["schema_hash"], capabilities["schema_hash"]);
    assert!(schema.get("process_schema").is_none());
}

#[tokio::test]
async fn health_and_source_are_stable() {
    let app = api::router(state());
    assert_eq!(json_response(app.clone(), "/health").await.0, StatusCode::OK);
    let (_, source) = json_response(app, "/source").await;
    assert_eq!(source["license"], "AGPL-3.0-or-later");
}
