use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CONTRACT_VERSION: &str = "1";
pub const ORCA_VERSION: &str = "2.4.2";
pub const ORCA_COMMIT: &str = "8500fcdccaa10b5099ac20d252af3a7c560046f1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub contract_version: String,
    pub engine: Engine,
    pub image_identity: ImageIdentity,
    pub schema_hash: String,
    pub capabilities: Capabilities,
    pub supported_scopes: Vec<String>,
    pub process_schema: ProcessSchema,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    pub name: String,
    pub version: String,
    pub commit: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImageIdentity {
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub process_schema: bool,
    pub model_state: bool,
    pub progress: bool,
    pub cancel: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessSchema {
    pub pages: Vec<Page>,
    pub options: Vec<Value>,
    pub scopes: BTreeMap<String, ScopeValue>,
    pub samples: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub name: String,
    pub groups: Vec<Group>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub options: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ScopeValue {
    One(String),
    Many(Vec<String>),
}

pub fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), canonical_json(v)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}
pub fn compact_json(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&canonical_json(value))
}
pub fn sha256_json(value: &Value) -> Result<String, serde_json::Error> {
    let mut h = Sha256::new();
    h.update(compact_json(value)?);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
pub fn process_hash(contract: &Contract) -> Result<String, serde_json::Error> {
    let p = serde_json::json!({"pages":contract.process_schema.pages,"options":contract.process_schema.options,"scopes":contract.process_schema.scopes,"samples":contract.process_schema.samples});
    sha256_json(&p)
}
pub fn validate_bounds(value: &Value) -> Result<(), String> {
    bounds(value, 0)
}
fn bounds(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err("maximum depth is 16".into());
    }
    match value {
        Value::Object(m) => {
            if m.len() > 4096 {
                return Err("maximum object entries is 4096".into());
            }
            for v in m.values() {
                bounds(v, depth + 1)?;
            }
        }
        Value::Array(a) => {
            if a.len() > 4096 {
                return Err("maximum array entries is 4096".into());
            }
            for v in a {
                bounds(v, depth + 1)?;
            }
        }
        Value::String(s) if s.len() > 16 * 1024 => {
            return Err("maximum string length is 16KiB".into());
        }
        _ => {}
    }
    Ok(())
}
pub fn parse_contract(input: &[u8]) -> Result<Contract, String> {
    let value: Value = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    validate_bounds(&value)?;
    let c: Contract = serde_json::from_value(value).map_err(|e| e.to_string())?;
    crate::schema::validate(&c)?;
    Ok(c)
}
