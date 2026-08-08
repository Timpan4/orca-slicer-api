use orca_slicer_api::{
    contract::{
        Capabilities, Contract, Engine, Group, ImageIdentity, Page, ProcessSchema, ScopeValue,
        process_hash,
    },
    model_state::validate_model_state,
};
use serde_json::json;
use std::collections::BTreeMap;

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
            model_state: true,
            progress: false,
            cancel: false,
        },
        supported_scopes: vec!["global".into(), "object".into()],
        process_schema: ProcessSchema {
            pages: vec![Page {
                name: "Quality".into(),
                groups: vec![Group {
                    name: "Overrides".into(),
                    options: vec!["temperature".into(), "quality".into(), "global_only".into()],
                }],
            }],
            options: vec![
                json!({"key":"temperature","type":"int","min":0,"max":300,"default":200}),
                json!({"key":"quality","type":"enum","choices":["draft","fine"],"default":"fine"}),
                json!({"key":"global_only","type":"bool","default":false}),
            ],
            scopes: BTreeMap::from([
                ("temperature".into(), ScopeValue::Many(vec!["global".into(), "object".into()])),
                ("quality".into(), ScopeValue::Many(vec!["global".into(), "object".into()])),
                ("global_only".into(), ScopeValue::One("global".into())),
            ]),
            samples: BTreeMap::from([
                ("temperature".into(), json!(200)),
                ("quality".into(), json!("fine")),
                ("global_only".into(), json!(false)),
            ]),
        },
    };
    contract.schema_hash = process_hash(&contract).unwrap();
    contract
}

#[test]
fn object_overrides_follow_schema_scope_type_range_and_choices() {
    let contract = contract();
    let valid = json!({
        "objects": [{
            "id": "1",
            "overrides": {"temperature": 220, "quality": "draft"}
        }]
    });
    validate_model_state(&valid, &contract).unwrap();

    for (key, value, expected) in [
        ("missing", json!(1), "unknown object override"),
        ("global_only", json!(true), "not object-scoped"),
        ("temperature", json!("hot"), "invalid value type"),
        ("temperature", json!(301), "out of range"),
        ("quality", json!("ultra"), "not an allowed choice"),
    ] {
        let state = json!({"objects":[{"id":"1","overrides":{(key):value}}]});
        assert!(
            validate_model_state(&state, &contract).unwrap_err().to_string().contains(expected)
        );
    }
}
