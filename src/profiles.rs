use crate::contract::parse_contract;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub fn load_json(path: impl AsRef<Path>) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    crate::contract::validate_bounds(&value)?;
    Ok(value)
}
pub fn load_profile_index(path: impl AsRef<Path>) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let object = value.as_object().ok_or("profile index must be an object")?;
    for key in ["printer", "process", "filament"] {
        let entries = object
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("profile index {key} must be an array"))?;
        for entry in entries {
            let entry = entry
                .as_object()
                .ok_or_else(|| format!("profile index {key} entry must be an object"))?;
            if entry.get("name").and_then(Value::as_str).is_none()
                || entry.get("base_id").and_then(Value::as_str).is_none()
            {
                return Err(format!("profile index {key} entries require name and base_id"));
            }
        }
    }
    Ok(value)
}

pub fn load_bridge_json(path: impl AsRef<Path>) -> Result<Value, String> {
    load_json(path)
}
pub fn load_process_profile_json(path: impl AsRef<Path>) -> Result<Value, String> {
    load_json(path)
}
pub fn load_typed<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T, String> {
    serde_json::from_value(load_json(path)?).map_err(|e| e.to_string())
}
pub fn load_contract(path: impl AsRef<Path>) -> Result<crate::contract::Contract, String> {
    parse_contract(&fs::read(path).map_err(|e| e.to_string())?)
}
