use crate::contract::parse_contract;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{collections::HashMap, fs, path::Path};

pub type ProfileCatalog = HashMap<(String, String), Vec<u8>>;

pub fn load_profile_catalog(root: impl AsRef<Path>) -> Result<ProfileCatalog, String> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut catalog = HashMap::new();
    collect_profile_files(&root, &root, &mut catalog)?;
    Ok(catalog)
}

fn collect_profile_files(
    root: &Path,
    directory: &Path,
    catalog: &mut ProfileCatalog,
) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_profile_files(root, &path, catalog)?;
        } else if file_type.is_symlink() {
            return Err(format!("profile source contains symlink: {}", path.display()));
        } else if file_type.is_file() && path.extension().is_some_and(|x| x == "json") {
            let canonical = fs::canonicalize(&path).map_err(|e| e.to_string())?;
            if !canonical.starts_with(root) {
                return Err(format!("profile escapes source root: {}", path.display()));
            }
            let bytes = fs::read(&canonical).map_err(|e| e.to_string())?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            let Some(profile_type) = value.get("type").and_then(Value::as_str) else {
                continue;
            };
            if !matches!(profile_type, "machine" | "process" | "filament")
                || value.get("instantiation").and_then(Value::as_str) == Some("false")
            {
                continue;
            }
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .ok_or_else(|| format!("{}: profile name required", path.display()))?;
            if catalog.insert((profile_type.into(), name.into()), bytes).is_some() {
                return Err(format!("duplicate {profile_type} profile name: {name}"));
            }
        } else if !file_type.is_file() {
            return Err(format!("profile source contains non-regular file: {}", path.display()));
        }
    }
    Ok(())
}

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
