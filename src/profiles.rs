use crate::contract::parse_contract;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

#[derive(Debug, Default)]
pub struct ProfileCatalog {
    records: Vec<ProfileRecord>,
    active: HashMap<(String, String), usize>,
    scoped: HashMap<(String, String, String), Vec<usize>>,
    global: HashMap<(String, String), Vec<usize>>,
}

#[derive(Debug)]
struct ProfileRecord {
    namespace: String,
    profile_type: String,
    name: String,
    bytes: Vec<u8>,
    value: Map<String, Value>,
}

impl ProfileCatalog {
    pub fn len(&self) -> usize {
        self.active.len()
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub fn resolve(&self, profile_type: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(&index) = self.active.get(&(profile_type.into(), name.into())) else {
            return Ok(None);
        };
        let record = &self.records[index];
        match record.value.get("inherits") {
            None | Some(Value::Null) => return Ok(Some(record.bytes.clone())),
            Some(Value::String(parent)) if parent.is_empty() => {
                return Ok(Some(record.bytes.clone()));
            }
            Some(Value::String(_)) => {}
            Some(_) => return Err(format!("bundled {profile_type} profile has invalid inherits")),
        }
        let value = self.resolve_object(index, &mut HashSet::new())?;
        serde_json::to_vec(&Value::Object(value)).map(Some).map_err(|error| error.to_string())
    }

    fn resolve_object(
        &self,
        index: usize,
        visiting: &mut HashSet<usize>,
    ) -> Result<Map<String, Value>, String> {
        if !visiting.insert(index) {
            let record = &self.records[index];
            return Err(format!(
                "bundled {} profile inheritance cycle: {}",
                record.profile_type, record.name
            ));
        }
        let result = (|| {
            let record = &self.records[index];
            let parent = match record.value.get("inherits") {
                None | Some(Value::Null) => return Ok(record.value.clone()),
                Some(Value::String(parent)) if parent.is_empty() => {
                    return Ok(record.value.clone());
                }
                Some(Value::String(parent)) => parent,
                Some(_) => return Err("bundled profile has invalid inherits".into()),
            };
            let parent_index = self.parent_index(record, parent)?;
            let mut merged = self.resolve_object(parent_index, visiting)?;
            merged.remove("instantiation");
            for (key, value) in &record.value {
                merged.insert(key.clone(), value.clone());
            }
            Ok(merged)
        })();
        visiting.remove(&index);
        result
    }

    fn parent_index(&self, record: &ProfileRecord, parent: &str) -> Result<usize, String> {
        let scoped = self
            .scoped
            .get(&(record.namespace.clone(), record.profile_type.clone(), parent.into()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match scoped {
            [index] => return Ok(*index),
            [] => {}
            _ => {
                return Err(format!(
                    "ambiguous bundled {} parent profile: {parent}",
                    record.profile_type
                ));
            }
        }
        let global = self
            .global
            .get(&(record.profile_type.clone(), parent.into()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match global {
            [index] => Ok(*index),
            [] => Err(format!("missing bundled {} parent profile: {parent}", record.profile_type)),
            _ => Err(format!("ambiguous bundled {} parent profile: {parent}", record.profile_type)),
        }
    }
}

pub fn load_profile_catalog(root: impl AsRef<Path>) -> Result<ProfileCatalog, String> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut catalog = ProfileCatalog::default();
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
            if !matches!(profile_type, "machine" | "process" | "filament") {
                continue;
            }
            let active = value.get("instantiation").and_then(Value::as_str) != Some("false");
            let Some(name) = value.get("name").and_then(Value::as_str).filter(|x| !x.is_empty())
            else {
                if active {
                    return Err(format!("{}: profile name required", path.display()));
                }
                continue;
            };
            let relative = canonical.strip_prefix(root).map_err(|e| e.to_string())?;
            let components = relative.components().collect::<Vec<_>>();
            let namespace = if components.len() > 1 {
                components[0].as_os_str().to_string_lossy().into_owned()
            } else {
                String::new()
            };
            let object = value
                .as_object()
                .ok_or_else(|| format!("{}: profile must be an object", path.display()))?
                .clone();
            let index = catalog.records.len();
            catalog.records.push(ProfileRecord {
                namespace: namespace.clone(),
                profile_type: profile_type.into(),
                name: name.into(),
                bytes,
                value: object,
            });
            catalog
                .scoped
                .entry((namespace, profile_type.into(), name.into()))
                .or_default()
                .push(index);
            catalog.global.entry((profile_type.into(), name.into())).or_default().push(index);
            if active && catalog.active.insert((profile_type.into(), name.into()), index).is_some()
            {
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
