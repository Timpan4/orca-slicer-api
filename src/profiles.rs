use crate::contract::parse_contract;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
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
    paths: HashMap<String, usize>,
    manifest: HashMap<(String, String, String), usize>,
    manifest_active: Vec<usize>,
    manifests_loaded: bool,
}

#[derive(Debug)]
struct ProfileRecord {
    namespace: String,
    scope: String,
    profile_type: String,
    name: String,
    base_id: String,
    file_stem: String,
    manifest_order: Option<usize>,
    bytes: Vec<u8>,
    value: Map<String, Value>,
}

impl ProfileCatalog {
    pub fn len(&self) -> usize {
        if self.manifests_loaded { self.manifest_active.len() } else { self.active.len() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn base_index(&self) -> Value {
        let indices = if self.manifests_loaded {
            self.manifest_active.clone()
        } else {
            self.active.values().copied().collect()
        };
        let mut index = Map::from_iter([
            ("printer".into(), Value::Array(Vec::new())),
            ("process".into(), Value::Array(Vec::new())),
            ("filament".into(), Value::Array(Vec::new())),
        ]);
        for record_index in indices {
            let record = &self.records[record_index];
            let slot = match record.profile_type.as_str() {
                "machine" => "printer",
                "process" => "process",
                "filament" => "filament",
                _ => continue,
            };
            index[slot].as_array_mut().unwrap().push(Value::Object(Map::from_iter([
                ("name".into(), Value::String(record.name.clone())),
                ("base_id".into(), Value::String(record.base_id.clone())),
            ])));
        }
        for entries in index.values_mut().filter_map(Value::as_array_mut) {
            entries.sort_by(|left, right| {
                left["name"]
                    .as_str()
                    .cmp(&right["name"].as_str())
                    .then(left["base_id"].as_str().cmp(&right["base_id"].as_str()))
            });
        }
        Value::Object(index)
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

    fn resolve_value(
        &self,
        profile_type: &str,
        name: &str,
    ) -> Result<Option<Map<String, Value>>, String> {
        let Some(&index) = self.active.get(&(profile_type.into(), name.into())) else {
            return Ok(None);
        };
        let record = &self.records[index];
        match record.value.get("inherits") {
            None | Some(Value::Null) => return Ok(Some(record.value.clone())),
            Some(Value::String(parent)) if parent.is_empty() => {
                return Ok(Some(record.value.clone()));
            }
            Some(Value::String(_)) => {}
            Some(_) => return Err(format!("bundled {profile_type} profile has invalid inherits")),
        }
        self.resolve_object(index, &mut HashSet::new()).map(Some)
    }

    pub fn decorate_index(&self, index: Value) -> Result<Value, String> {
        let mut index = index;
        let slots = [("printer", "machine"), ("process", "process"), ("filament", "filament")];
        for (slot, profile_type) in slots {
            let entries = index
                .get_mut(slot)
                .and_then(Value::as_array_mut)
                .ok_or_else(|| format!("profile index {slot} must be an array"))?;
            for entry in entries {
                let object = entry
                    .as_object_mut()
                    .ok_or_else(|| format!("profile index {slot} entry must be an object"))?;
                let name = object.get("name").and_then(Value::as_str).ok_or_else(|| {
                    format!("profile index {slot} entries require name and base_id")
                })?;
                let base_id = object.get("base_id").and_then(Value::as_str).ok_or_else(|| {
                    format!("profile index {slot} entries require name and base_id")
                })?;
                let record_index = *self
                    .active
                    .get(&(profile_type.into(), name.into()))
                    .ok_or_else(|| format!("missing bundled {profile_type} profile: {name}"))?;
                let record = &self.records[record_index];
                if record.base_id != base_id {
                    return Err(format!(
                        "profile index {slot} entry does not match bundled source: {name}"
                    ));
                }
                let resolved = self
                    .resolve_value(profile_type, name)
                    .map_err(|error| {
                        format!("failed to resolve bundled {profile_type} profile {name}: {error}")
                    })?
                    .ok_or_else(|| format!("missing bundled {profile_type} profile: {name}"))?;
                let resolved_value = Value::Object(resolved);
                let canonical = canonical_json(&resolved_value);
                let hash = Sha256::digest(&canonical);
                object.insert(
                    "stable_id".into(),
                    Value::String(format!("{slot}:{}:{}", record.namespace, record.base_id)),
                );
                object.insert("content".into(), resolved_value.clone());
                object.insert("content_hash".into(), Value::String(hex_hash(&hash)));
                if let Some(compatible) = resolved_value.get("compatible_printers")
                    && compatible.is_array()
                {
                    object.insert("compatible_printers".into(), compatible.clone());
                }
            }
        }
        Ok(index)
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

    fn select_parent(
        &self,
        candidates: &[usize],
        profile_type: &str,
        parent: &str,
    ) -> Result<Option<usize>, String> {
        match candidates {
            [] => Ok(None),
            [index] => Ok(Some(*index)),
            _ => {
                let exact = candidates
                    .iter()
                    .copied()
                    .filter(|index| self.records[*index].file_stem == parent)
                    .collect::<Vec<_>>();
                match exact.as_slice() {
                    [index] => Ok(Some(*index)),
                    _ => Err(format!("ambiguous bundled {profile_type} parent profile: {parent}")),
                }
            }
        }
    }

    fn parent_index(&self, record: &ProfileRecord, parent: &str) -> Result<usize, String> {
        if let Some(order) = record.manifest_order {
            let key = (record.namespace.clone(), record.profile_type.clone(), parent.into());
            if let Some(&index) = self.manifest.get(&key)
                && self.records[index]
                    .manifest_order
                    .is_some_and(|parent_order| parent_order < order)
            {
                return Ok(index);
            }
            if record.profile_type == "filament"
                && record.namespace != "OrcaFilamentLibrary"
                && let Some(&index) = self.manifest.get(&(
                    "OrcaFilamentLibrary".into(),
                    record.profile_type.clone(),
                    parent.into(),
                ))
            {
                return Ok(index);
            }
            return Err(format!(
                "missing bundled {} parent profile: {parent}",
                record.profile_type
            ));
        }
        if self.manifests_loaded {
            return Err(format!(
                "bundled {} profile is not declared by its manifest",
                record.profile_type
            ));
        }
        let mut scope = record.scope.as_str();
        loop {
            let scoped = self
                .scoped
                .get(&(scope.into(), record.profile_type.clone(), parent.into()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            if let Some(index) = self.select_parent(scoped, &record.profile_type, parent)? {
                return Ok(index);
            }
            if scope.is_empty() {
                break;
            }
            scope = scope.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
        let global = self
            .global
            .get(&(record.profile_type.clone(), parent.into()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        match self.select_parent(global, &record.profile_type, parent)? {
            Some(index) => Ok(index),
            None => {
                Err(format!("missing bundled {} parent profile: {parent}", record.profile_type))
            }
        }
    }
}

pub fn load_profile_catalog(root: impl AsRef<Path>) -> Result<ProfileCatalog, String> {
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut catalog = ProfileCatalog::default();
    collect_profile_files(&root, &root, &mut catalog)?;
    load_profile_manifests(&root, &mut catalog)?;
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
            let scope = relative
                .parent()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let file_stem = relative
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| format!("profile filename required: {}", path.display()))?;
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
            let relative_path = relative.to_string_lossy().replace('\\', "/");
            let base_id = object
                .get("setting_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map_or_else(|| relative_path.clone(), str::to_owned);
            let index = catalog.records.len();
            catalog.records.push(ProfileRecord {
                namespace: namespace.clone(),
                scope: scope.clone(),
                profile_type: profile_type.into(),
                name: name.into(),
                base_id,
                file_stem,
                manifest_order: None,
                bytes,
                value: object,
            });
            catalog.paths.insert(relative_path, index);
            catalog
                .scoped
                .entry((scope, profile_type.into(), name.into()))
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

fn load_profile_manifests(root: &Path, catalog: &mut ProfileCatalog) -> Result<(), String> {
    let mut entries = fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut active = HashSet::new();
    for entry in entries {
        let path = entry.path();
        if !entry.file_type().map_err(|error| error.to_string())?.is_file()
            || path.extension().is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let value: Value =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        let namespace = path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| format!("manifest filename required: {}", path.display()))?;
        for (list, profile_type) in [
            ("machine_list", "machine"),
            ("process_list", "process"),
            ("filament_list", "filament"),
        ] {
            let Some(manifest_entries) = value.get(list) else {
                continue;
            };
            let manifest_entries = manifest_entries
                .as_array()
                .ok_or_else(|| format!("{}: {list} must be an array", path.display()))?;
            catalog.manifests_loaded = true;
            for (order, manifest_entry) in manifest_entries.iter().enumerate() {
                let manifest_entry = manifest_entry
                    .as_object()
                    .ok_or_else(|| format!("{}: {list} entry must be an object", path.display()))?;
                let name = manifest_entry
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| format!("{}: {list} entry name required", path.display()))?;
                let sub_path = manifest_entry
                    .get("sub_path")
                    .and_then(Value::as_str)
                    .filter(|sub_path| !sub_path.is_empty())
                    .ok_or_else(|| format!("{}: {list} entry sub_path required", path.display()))?
                    .replace('\\', "/");
                if sub_path.starts_with('/')
                    || sub_path.split('/').any(|component| matches!(component, "" | "." | ".."))
                {
                    return Err(format!(
                        "{}: invalid manifest sub_path: {sub_path}",
                        path.display()
                    ));
                }
                let relative = format!("{namespace}/{sub_path}");
                let index = *catalog.paths.get(&relative).ok_or_else(|| {
                    format!("{}: manifest profile not found: {relative}", path.display())
                })?;
                let record = &mut catalog.records[index];
                if record.namespace != namespace
                    || record.profile_type != profile_type
                    || record.name != name
                {
                    return Err(format!(
                        "{}: manifest profile does not match: {relative}",
                        path.display()
                    ));
                }
                record.manifest_order = Some(order);
                let key = (namespace.clone(), profile_type.into(), name.into());
                if let Some(previous) = catalog.manifest.insert(key, index)
                    && previous != index
                {
                    return Err(format!(
                        "{}: duplicate {profile_type} manifest profile: {name}",
                        path.display()
                    ));
                }
                if record.value.get("instantiation").and_then(Value::as_str) != Some("false")
                    && active.insert(index)
                {
                    catalog.manifest_active.push(index);
                }
            }
        }
    }
    Ok(())
}

fn hex_hash(hash: &[u8]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonical_json(value: &Value) -> Vec<u8> {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries = object.iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                Value::Object(
                    entries.into_iter().map(|(key, value)| (key.clone(), sort(value))).collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(sort).collect()),
            value => value.clone(),
        }
    }
    serde_json::to_vec(&sort(value)).expect("JSON values are serializable")
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
