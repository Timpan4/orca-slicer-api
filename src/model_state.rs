use crate::{
    contract::{Contract, ScopeValue},
    error::AppError,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const ID: &str = r"^[A-Za-z0-9_.:-]{1,128}$";
const OVERRIDE: &str = r"^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$";

pub fn validate_model_state(v: &Value, contract: &Contract) -> Result<(), AppError> {
    if serde_json::to_vec(v).map_err(|_| AppError::Bad("invalid modelState".into()))?.len()
        > 256 * 1024
    {
        return Err(AppError::Bad("modelState exceeds 256KiB".into()));
    }
    let root = v.as_object().ok_or_else(|| AppError::Bad("modelState must be an object".into()))?;
    let objects = root
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Bad("modelState.objects is required".into()))?;
    if objects.len() > 256 {
        return Err(AppError::Bad("too many model objects".into()));
    }
    let id_re = regex::Regex::new(ID).unwrap();
    let override_re = regex::Regex::new(OVERRIDE).unwrap();
    let options = contract
        .process_schema
        .options
        .iter()
        .filter_map(|option| option.get("key").and_then(Value::as_str).map(|key| (key, option)))
        .collect::<HashMap<_, _>>();
    let mut ids = HashSet::new();
    for object in objects {
        let o = object
            .as_object()
            .ok_or_else(|| AppError::Bad("model objects must be objects".into()))?;
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Bad("model object id is required".into()))?;
        if !id_re.is_match(id) || !ids.insert(id) {
            return Err(AppError::Bad("object ids must be unique safe IDs".into()));
        }
        if let Some(transform) = o.get("transform") {
            let transform = transform
                .as_object()
                .ok_or_else(|| AppError::Bad("transform must be an object".into()))?;
            for key in ["position", "rotation"] {
                if let Some(vector) = transform.get(key) {
                    validate_vector(vector, false, key)?;
                }
            }
            if let Some(vector) = transform.get("scale") {
                validate_vector(vector, true, "scale")?;
            }
            if transform
                .keys()
                .any(|key| !matches!(key.as_str(), "position" | "rotation" | "scale"))
            {
                return Err(AppError::Bad("transform contains unknown field".into()));
            }
        }
        if let Some(overrides) = o.get("overrides") {
            let map = overrides
                .as_object()
                .ok_or_else(|| AppError::Bad("overrides must be an object".into()))?;
            if serde_json::to_vec(map)
                .map_err(|_| AppError::Bad("invalid object overrides".into()))?
                .len()
                > 32 * 1024
            {
                return Err(AppError::Bad("object overrides exceed 32KiB".into()));
            }
            for (key, value) in map {
                if !override_re.is_match(key) || !scalar_or_list(value) {
                    return Err(AppError::Bad("invalid object override".into()));
                }
                let option = options
                    .get(key.as_str())
                    .ok_or_else(|| AppError::Bad(format!("unknown object override: {key}")))?;
                let object_scope = matches!(
                    contract.process_schema.scopes.get(key),
                    Some(ScopeValue::Many(scopes)) if scopes.iter().any(|scope| scope == "object")
                );
                if !object_scope {
                    return Err(AppError::Bad(format!("override is not object-scoped: {key}")));
                }
                validate_override_value(key, option, value)?;
                if serde_json::to_vec(value).unwrap_or_default().len() > 16 * 1024 {
                    return Err(AppError::Bad("object override too large".into()));
                }
            }
        }
    }
    for key in ["hidden_object_ids", "lay_flat_object_ids"] {
        if let Some(value) = root.get(key) {
            let a =
                value.as_array().ok_or_else(|| AppError::Bad(format!("{key} must be an array")))?;
            let mut seen = HashSet::new();
            for id in a {
                let id = id
                    .as_str()
                    .ok_or_else(|| AppError::Bad(format!("{key} must contain strings")))?;
                if !ids.contains(id) || !seen.insert(id) {
                    return Err(AppError::Bad(format!("unknown or duplicate {key}")));
                }
            }
        }
    }
    if let Some(arrange) = root.get("arrange")
        && !arrange.is_boolean()
    {
        return Err(AppError::Bad("arrange must be boolean".into()));
    }
    Ok(())
}

fn validate_vector(value: &Value, positive: bool, name: &str) -> Result<(), AppError> {
    let vector =
        value.as_array().ok_or_else(|| AppError::Bad(format!("{name} must be an array")))?;
    if vector.len() != 3
        || vector.iter().any(|entry| {
            entry.as_f64().is_none_or(|number| !number.is_finite() || (positive && number <= 0.0))
        })
    {
        return Err(AppError::Bad(format!("{name} must contain three valid numbers")));
    }
    Ok(())
}

fn validate_override_value(key: &str, option: &Value, value: &Value) -> Result<(), AppError> {
    if value.is_null() {
        return if option.get("nullable").and_then(Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(AppError::Bad(format!("override is not nullable: {key}")))
        };
    }
    let option_type = option
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Bad(format!("override metadata missing type: {key}")))?;
    let valid_type = match option_type {
        "bool" | "boolean" => value.is_boolean(),
        "int" | "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "float" | "number" | "percent" => value.is_number(),
        "enum" => value.is_string() || value.is_number(),
        "array" => value.is_array(),
        "string" | "float_or_percent" | "point" | "point3" => value.is_string(),
        _ => false,
    };
    if !valid_type {
        return Err(AppError::Bad(format!("invalid value type for object override: {key}")));
    }
    if let Some(number) = value.as_f64()
        && (option.get("min").and_then(Value::as_f64).is_some_and(|minimum| number < minimum)
            || option.get("max").and_then(Value::as_f64).is_some_and(|maximum| number > maximum))
    {
        return Err(AppError::Bad(format!("object override is out of range: {key}")));
    }
    if let Some(choices) = option.get("choices").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return Err(AppError::Bad(format!("object override is not an allowed choice: {key}")));
    }
    Ok(())
}

fn scalar_or_list(v: &Value) -> bool {
    match v {
        Value::String(_) | Value::Bool(_) | Value::Number(_) | Value::Null => true,
        Value::Array(a) => a.iter().all(scalar_or_list) && a.len() <= 256,
        _ => false,
    }
}
