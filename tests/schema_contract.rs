use orca_slicer_api::{
    contract::{
        self, Capabilities, Contract, Engine, Group, ImageIdentity, Page, ProcessSchema,
        ScopeValue, process_hash,
    },
    schema,
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
            options: vec![json!({"key":"layer_height","type":"number"})],
            scopes: BTreeMap::from([("layer_height".into(), ScopeValue::One("global".into()))]),
            samples: BTreeMap::from([("layer_height".into(), json!(0.2))]),
        },
    };
    contract.schema_hash = process_hash(&contract).unwrap();
    contract
}

#[test]
fn canonical_hash_is_order_independent() {
    let left = json!({"b": 2, "a": 1});
    let right = json!({"a": 1, "b": 2});
    assert_eq!(contract::compact_json(&left).unwrap(), br#"{"a":1,"b":2}"#);
    assert_eq!(contract::sha256_json(&left).unwrap(), contract::sha256_json(&right).unwrap());
}

#[test]
fn bounds_reject_deep_json() {
    let mut value = json!(null);
    for _ in 0..17 {
        value = json!([value]);
    }
    assert!(contract::validate_bounds(&value).is_err());
}

#[test]
fn loaded_contract_rejects_any_non_runtime_capability_tuple() {
    for capabilities in [
        Capabilities { process_schema: false, model_state: false, progress: true, cancel: false },
        Capabilities { process_schema: true, model_state: true, progress: true, cancel: false },
        Capabilities { process_schema: true, model_state: false, progress: false, cancel: false },
        Capabilities { process_schema: true, model_state: false, progress: true, cancel: true },
    ] {
        let mut value = serde_json::to_value(contract()).unwrap();
        value["capabilities"] = serde_json::to_value(capabilities).unwrap();
        assert!(contract::parse_contract(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
#[test]
fn schema_rejects_unsupported_option_types() {
    let mut unsupported = contract();
    unsupported.process_schema.options[0]["type"] = json!("unknown");
    unsupported.schema_hash = process_hash(&unsupported).unwrap();
    assert!(schema::validate(&unsupported).unwrap_err().contains("unsupported option type"));
}

#[test]
fn schema_requires_scope_and_sample_for_every_option() {
    let mut missing_scope = contract();
    missing_scope.process_schema.scopes.clear();
    missing_scope.schema_hash = process_hash(&missing_scope).unwrap();
    assert!(schema::validate(&missing_scope).unwrap_err().contains("missing scope"));

    let mut missing_sample = contract();
    missing_sample.process_schema.samples.clear();
    missing_sample.schema_hash = process_hash(&missing_sample).unwrap();
    assert!(schema::validate(&missing_sample).unwrap_err().contains("missing sample"));
}
