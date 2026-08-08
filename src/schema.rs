use crate::contract::{
    CONTRACT_VERSION, Contract, Group, ORCA_COMMIT, ORCA_VERSION, Page, ProcessSchema, ScopeValue,
    process_hash,
};
use serde_json::{Map, Number, Value, json};
use std::collections::{BTreeMap, HashSet};

pub struct OrcaSources<'a> {
    pub print_config: &'a str,
    pub print_config_header: &'a str,
    pub constants: &'a str,
}

#[derive(Clone)]
struct RawOption {
    option_type: String,
    block: String,
    nullable: bool,
}

struct ParsedSource {
    options: BTreeMap<String, RawOption>,
    aliases: BTreeMap<String, String>,
    constants: BTreeMap<String, String>,
    enum_values: BTreeMap<String, BTreeMap<String, String>>,
    object_keys: HashSet<String>,
    duplicate_options: HashSet<String>,
}

type OptionBlocks = (BTreeMap<String, RawOption>, BTreeMap<String, String>, HashSet<String>);

pub fn build_process_schema(
    sources: OrcaSources<'_>,
    tab_print_source: &str,
) -> Result<ProcessSchema, String> {
    let pages = extract_tab_print_layout(tab_print_source)?;
    let placed = pages
        .iter()
        .flat_map(|page| &page.groups)
        .flat_map(|group| &group.options)
        .cloned()
        .collect::<Vec<_>>();
    let mut unique = HashSet::new();
    if let Some(key) = placed.iter().find(|key| !unique.insert(key.as_str())) {
        return Err(format!("duplicate placed option: {key}"));
    }
    let parsed = parse_source(sources)?;
    let mut options = Vec::with_capacity(placed.len());
    let mut scopes = BTreeMap::new();
    let mut samples = BTreeMap::new();
    for key in placed {
        let option = option_json(&key, &parsed)?;
        let sample = option
            .get("default")
            .ok_or_else(|| format!("source option missing default: {key}"))?
            .clone();
        if sample.is_null() && option.get("nullable").and_then(Value::as_bool) != Some(true) {
            return Err(format!("non-nullable source option has null default: {key}"));
        }
        options.push(option);
        scopes.insert(
            key.clone(),
            if parsed.object_keys.contains(&key) {
                ScopeValue::Many(vec!["global".into(), "object".into()])
            } else {
                ScopeValue::One("global".into())
            },
        );
        samples.insert(key, sample);
    }
    Ok(ProcessSchema { pages, options, scopes, samples })
}

fn parse_source(sources: OrcaSources<'_>) -> Result<ParsedSource, String> {
    let print_config = strip_cpp_comments(sources.print_config)?;
    let header = strip_cpp_comments(sources.print_config_header)?;
    let (options, aliases, duplicate_options) = parse_option_blocks(&print_config)?;
    let mut constants = parse_constants(sources.constants);
    constants.extend(parse_constants(&print_config));
    Ok(ParsedSource {
        options,
        aliases,
        constants,
        enum_values: parse_enum_maps(&print_config)?,
        object_keys: parse_object_keys(&header)?,
        duplicate_options,
    })
}

fn parse_option_blocks(source: &str) -> Result<OptionBlocks, String> {
    let call_pattern =
        regex::Regex::new(r"this->add(_nullable)?\s*\(").map_err(|error| error.to_string())?;
    let literal_pattern = regex::Regex::new(
        r#"^this->add(_nullable)?\s*\(\s*"([A-Za-z0-9_]+)"\s*,\s*(co[A-Za-z0-9_]+)"#,
    )
    .map_err(|error| error.to_string())?;
    let alias_pattern = regex::Regex::new(r"auto\s+([A-Za-z0-9_]+)\s*=\s*def\s*=\s*$")
        .map_err(|error| error.to_string())?;
    let starts = call_pattern.find_iter(source).map(|item| item.start()).collect::<Vec<_>>();
    let mut options = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    let mut duplicate_options = HashSet::new();
    for (index, start) in starts.iter().copied().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(source.len());
        let block = &source[start..end];
        let Some(captures) = literal_pattern.captures(block) else {
            continue;
        };
        let key = captures[2].to_owned();
        let option = RawOption {
            option_type: captures[3].to_owned(),
            block: block.to_owned(),
            nullable: captures.get(1).is_some(),
        };
        if options.insert(key.clone(), option).is_some() {
            duplicate_options.insert(key.clone());
        }
        let line_start = source[..start].rfind('\n').map_or(0, |position| position + 1);
        if let Some(captures) = alias_pattern.captures(&source[line_start..start]) {
            aliases.insert(captures[1].to_owned(), key);
        }
    }
    Ok((options, aliases, duplicate_options))
}

fn option_json(key: &str, parsed: &ParsedSource) -> Result<Value, String> {
    if parsed.duplicate_options.contains(key) {
        return Err(format!("duplicate placed source option: {key}"));
    }
    let option = parsed
        .options
        .get(key)
        .ok_or_else(|| format!("layout option missing source metadata: {key}"))?;
    let label = string_field(key, "full_label", parsed, &mut HashSet::new())?
        .or(string_field(key, "label", parsed, &mut HashSet::new())?)
        .ok_or_else(|| format!("source option missing label: {key}"))?;
    let tooltip = string_field(key, "tooltip", parsed, &mut HashSet::new())?.unwrap_or_default();
    let units = string_field(key, "sidetext", parsed, &mut HashSet::new())?;
    let mode = match assignment(&option.block, "mode") {
        None => "simple",
        Some(value) => match value.trim() {
            "comSimple" => "simple",
            "comAdvanced" => "advanced",
            "comExpert" | "comDevelop" => "expert",
            other => return Err(format!("unsupported option mode for {key}: {other}")),
        },
    };
    let nullable = assignment(&option.block, "nullable")
        .map(|value| parse_bool(value.trim(), &parsed.constants))
        .transpose()?
        .unwrap_or(option.nullable);
    let mut value = Map::from_iter([
        ("key".into(), Value::String(key.into())),
        ("type".into(), Value::String(schema_type(&option.option_type)?.into())),
        ("label".into(), Value::String(label)),
        ("tooltip".into(), Value::String(tooltip)),
        ("mode".into(), Value::String(mode.into())),
        ("units".into(), units.map_or(Value::Null, Value::String)),
        ("nullable".into(), Value::Bool(nullable)),
        ("default".into(), parse_default(key, option, parsed)?),
    ]);
    if let Some(minimum) = numeric_field(key, "min", parsed, &mut HashSet::new())? {
        value.insert("min".into(), number_value(minimum)?);
    }
    if let Some(maximum) = numeric_field(key, "max", parsed, &mut HashSet::new())? {
        value.insert("max".into(), number_value(maximum)?);
    }
    let choices = option_choices(key, parsed, &mut HashSet::new())?;
    if !choices.is_empty() {
        let choices = choices
            .into_iter()
            .map(|choice| match option.option_type.as_str() {
                "coFloat" | "coPercent" => {
                    Ok(number_value(parse_number(&choice, &parsed.constants)?)?)
                }
                "coInt" => {
                    let number = parse_number(&choice, &parsed.constants)?;
                    if number.fract() != 0.0 {
                        Err(format!("integer choice is not integral for {key}: {choice}"))
                    } else {
                        Ok(Value::Number(Number::from(number as i64)))
                    }
                }
                "coBool" => Ok(Value::Bool(parse_bool(&choice, &parsed.constants)?)),
                "coString" | "coEnum" | "coFloatOrPercent" => Ok(Value::String(choice)),
                _ => Err(format!(
                    "choices are unsupported for option type {}: {key}",
                    option.option_type
                )),
            })
            .collect::<Result<Vec<_>, String>>()?;
        value.insert("choices".into(), Value::Array(choices));
    }
    Ok(Value::Object(value))
}

fn schema_type(option_type: &str) -> Result<&'static str, String> {
    match option_type {
        "coFloat" => Ok("float"),
        "coInt" => Ok("int"),
        "coString" => Ok("string"),
        "coPercent" => Ok("percent"),
        "coFloatOrPercent" => Ok("float_or_percent"),
        "coBool" => Ok("bool"),
        "coEnum" => Ok("enum"),
        "coFloats" | "coInts" | "coStrings" | "coPercents" | "coFloatsOrPercents" | "coPoints"
        | "coBools" | "coEnums" | "coPointsGroups" | "coIntsGroups" => Ok("array"),
        other => Err(format!("unsupported source option type: {other}")),
    }
}

fn parse_default(key: &str, option: &RawOption, parsed: &ParsedSource) -> Result<Value, String> {
    let (constructor, arguments) = default_constructor(&option.block)
        .ok_or_else(|| format!("source option missing default: {key}"))?;
    match option.option_type.as_str() {
        "coFloat" | "coPercent" => number_value(parse_number(&arguments, &parsed.constants)?),
        "coInt" => {
            let number = parse_number(&arguments, &parsed.constants)?;
            if number.fract() != 0.0 {
                return Err(format!("integer default is not integral for {key}: {arguments}"));
            }
            Ok(Value::Number(Number::from(number as i64)))
        }
        "coBool" => Ok(Value::Bool(parse_bool(&arguments, &parsed.constants)?)),
        "coString" => Ok(Value::String(cpp_strings(&arguments)?.join(""))),
        "coStrings" => {
            Ok(serde_json::to_value(cpp_strings(&arguments)?).map_err(|e| e.to_string())?)
        }
        "coFloatOrPercent" => {
            let parts = split_top_level(&arguments, ',');
            if parts.len() != 2 {
                return Err(format!("invalid float-or-percent default for {key}: {arguments}"));
            }
            let number = parse_number(parts[0], &parsed.constants)?;
            let percent = parse_bool(parts[1], &parsed.constants)?;
            Ok(Value::String(format!("{number}{}", if percent { "%" } else { "" })))
        }
        "coEnum" => {
            let enum_type = constructor
                .strip_prefix("ConfigOptionEnum<")
                .and_then(|value| value.strip_suffix('>'))
                .ok_or_else(|| format!("invalid enum constructor for {key}: {constructor}"))?;
            let normalized = normalize_enum_value(&arguments);
            let value = parsed
                .enum_values
                .get(enum_type)
                .and_then(|values| values.get(&normalized))
                .ok_or_else(|| format!("unknown enum default for {key}: {arguments}"))?;
            Ok(Value::String(value.clone()))
        }
        other => Err(format!("unsupported default type for {key}: {other}")),
    }
}

fn default_constructor(block: &str) -> Option<(String, String)> {
    let marker = "set_default_value(new";
    let start = block.find(marker)? + marker.len();
    let rest = block[start..].trim_start();
    let delimiter = rest.find(['(', '{'])?;
    let constructor = rest[..delimiter].trim().to_owned();
    let open = rest.as_bytes()[delimiter] as char;
    let close = if open == '(' { ')' } else { '}' };
    let end = matching_delimiter(rest, delimiter, open, close)?;
    Some((constructor, rest[delimiter + 1..end].trim().to_owned()))
}

fn option_choices(
    key: &str,
    parsed: &ParsedSource,
    visiting: &mut HashSet<String>,
) -> Result<Vec<String>, String> {
    if !visiting.insert(key.into()) {
        return Err(format!("cyclic enum metadata reference: {key}"));
    }
    let option =
        parsed.options.get(key).ok_or_else(|| format!("unknown enum metadata option: {key}"))?;
    let mut values = Vec::new();
    if let Some(expression) = assignment(&option.block, "enum_values") {
        if let Some(alias) = referenced_field(expression, "enum_values") {
            let source_key = parsed
                .aliases
                .get(alias)
                .ok_or_else(|| format!("unknown enum metadata reference for {key}: {alias}"))?;
            values = option_choices(source_key, parsed, visiting)?;
        } else {
            values.extend(cpp_strings(expression)?);
        }
    }
    enum ChoiceOperation {
        Append(String),
        Value(String),
    }
    let append_pattern = regex::Regex::new(
        r#"append\s*\(\s*def->enum_values\s*,\s*([A-Za-z_][A-Za-z0-9_]*)->enum_values\s*\)"#,
    )
    .map_err(|error| error.to_string())?;
    let call_pattern = regex::Regex::new(
        r#"def->enum_values\.(?:push_back|emplace_back)\s*\(\s*("(?:\\.|[^"\\])*")\s*\)"#,
    )
    .map_err(|error| error.to_string())?;
    let mut operations = Vec::new();
    for captures in append_pattern.captures_iter(&option.block) {
        operations.push((
            captures.get(0).unwrap().start(),
            ChoiceOperation::Append(captures[1].to_owned()),
        ));
    }
    for captures in call_pattern.captures_iter(&option.block) {
        operations.push((
            captures.get(0).unwrap().start(),
            ChoiceOperation::Value(decode_cpp_string(&captures[1])?),
        ));
    }
    operations.sort_by_key(|(position, _)| *position);
    for (_, operation) in operations {
        match operation {
            ChoiceOperation::Append(alias) => {
                let source_key = parsed
                    .aliases
                    .get(&alias)
                    .ok_or_else(|| format!("unknown enum metadata reference for {key}: {alias}"))?;
                values.extend(option_choices(source_key, parsed, visiting)?);
            }
            ChoiceOperation::Value(value) => values.push(value),
        }
    }
    visiting.remove(key);
    Ok(values)
}

fn string_field(
    key: &str,
    field: &str,
    parsed: &ParsedSource,
    visiting: &mut HashSet<String>,
) -> Result<Option<String>, String> {
    if !visiting.insert(format!("{key}:{field}")) {
        return Err(format!("cyclic string metadata reference: {key}.{field}"));
    }
    let option =
        parsed.options.get(key).ok_or_else(|| format!("unknown string metadata option: {key}"))?;
    let result = if let Some(expression) = assignment(&option.block, field) {
        if let Some(alias) = referenced_field(expression, field) {
            let source_key = parsed
                .aliases
                .get(alias)
                .ok_or_else(|| format!("unknown string metadata reference for {key}: {alias}"))?;
            string_field(source_key, field, parsed, visiting)?
        } else {
            Some(cpp_strings(expression)?.join(""))
        }
    } else {
        None
    };
    visiting.remove(&format!("{key}:{field}"));
    Ok(result)
}

fn numeric_field(
    key: &str,
    field: &str,
    parsed: &ParsedSource,
    visiting: &mut HashSet<String>,
) -> Result<Option<f64>, String> {
    if !visiting.insert(format!("{key}:{field}")) {
        return Err(format!("cyclic numeric metadata reference: {key}.{field}"));
    }
    let option =
        parsed.options.get(key).ok_or_else(|| format!("unknown numeric metadata option: {key}"))?;
    let result = if let Some(expression) = assignment(&option.block, field) {
        if let Some(alias) = referenced_field(expression, field) {
            let source_key = parsed
                .aliases
                .get(alias)
                .ok_or_else(|| format!("unknown numeric metadata reference for {key}: {alias}"))?;
            numeric_field(source_key, field, parsed, visiting)?
        } else {
            Some(parse_number(expression, &parsed.constants)?)
        }
    } else {
        None
    };
    visiting.remove(&format!("{key}:{field}"));
    Ok(result)
}

fn referenced_field<'a>(expression: &'a str, field: &str) -> Option<&'a str> {
    let suffix = format!("->{field}");
    expression.trim().strip_suffix(&suffix).map(str::trim)
}

fn assignment<'a>(block: &'a str, field: &str) -> Option<&'a str> {
    let pattern = format!("def->{field}");
    let mut search = 0;
    let expression = loop {
        let start = search + block[search..].find(&pattern)? + pattern.len();
        let rest = &block[start..];
        let trimmed = rest.trim_start();
        if let Some(expression) = trimmed.strip_prefix('=') {
            break expression;
        }
        search = start;
    };
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0_i32;
    for (index, character) in expression.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            ';' if depth == 0 => return Some(expression[..index].trim()),
            _ => {}
        }
    }
    None
}

fn parse_constants(source: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let define = regex::Regex::new(r"(?m)^\s*#define\s+([A-Za-z0-9_]+)\s+([^\r\n]+)")
        .expect("constant regex");
    for captures in define.captures_iter(source) {
        values.insert(captures[1].into(), captures[2].trim().into());
    }
    let local = regex::Regex::new(
        r"(?m)\b(?:const|constexpr)\s+(?:int|float|double|bool)\s+([A-Za-z0-9_]+)\s*=\s*([^;]+);",
    )
    .expect("local constant regex");
    for captures in local.captures_iter(source) {
        values.insert(captures[1].into(), captures[2].trim().into());
    }
    values
}

fn parse_number(expression: &str, constants: &BTreeMap<String, String>) -> Result<f64, String> {
    let expression = expression.trim();
    let (negative, value) =
        expression.strip_prefix('-').map_or((false, expression), |value| (true, value));
    let resolved = constants.get(value).map_or(value, String::as_str).trim();
    let resolved = resolved.strip_suffix(['f', 'F']).unwrap_or(resolved);
    let number = resolved
        .parse::<f64>()
        .map_err(|_| format!("unsupported numeric expression: {expression}"))?;
    Ok(if negative { -number } else { number })
}

fn parse_bool(expression: &str, constants: &BTreeMap<String, String>) -> Result<bool, String> {
    let expression = expression.trim();
    match constants.get(expression).map_or(expression, String::as_str).trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(format!("unsupported boolean expression: {other}")),
    }
}

fn number_value(number: f64) -> Result<Value, String> {
    Number::from_f64(number).map(Value::Number).ok_or_else(|| "non-finite source number".into())
}

fn parse_enum_maps(source: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let pattern = regex::Regex::new(r"s_keys_map_([A-Za-z0-9_]+)\s*(?:=\s*)?\{")
        .map_err(|error| error.to_string())?;
    let mut maps = BTreeMap::new();
    for captures in pattern.captures_iter(source) {
        let whole = captures.get(0).ok_or("enum map match missing")?;
        let open = whole.end() - 1;
        let close = matching_delimiter(source, open, '{', '}')
            .ok_or_else(|| format!("unterminated enum map: {}", &captures[1]))?;
        let body = &source[open + 1..close];
        let mut values = BTreeMap::new();
        for entry in braced_entries(body) {
            let parts = split_top_level(entry, ',');
            if parts.len() < 2 {
                continue;
            }
            let names = cpp_strings(parts[0])?;
            if names.len() != 1 {
                continue;
            }
            values.insert(normalize_enum_value(parts[1]), names[0].clone());
        }
        maps.insert(captures[1].into(), values);
    }
    Ok(maps)
}

fn normalize_enum_value(expression: &str) -> String {
    let mut value =
        expression.chars().filter(|character| !character.is_whitespace()).collect::<String>();
    while let Some(inner) = value.strip_prefix("int(").and_then(|value| value.strip_suffix(')')) {
        value = inner.into();
    }
    value
}

fn parse_object_keys(source: &str) -> Result<HashSet<String>, String> {
    let object_start = source.find("PrintObjectConfig,").ok_or("PrintObjectConfig not found")?;
    let region_start = source.find("PrintRegionConfig,").ok_or("PrintRegionConfig not found")?;
    let region_end = source[region_start..]
        .find("PRINT_CONFIG_CLASS_DERIVED")
        .map(|offset| region_start + offset)
        .ok_or("end of PrintRegionConfig not found")?;
    if object_start >= region_start {
        return Err("PrintObjectConfig must precede PrintRegionConfig".into());
    }
    let pattern =
        regex::Regex::new(r"\(\(\s*ConfigOption[^,()]+(?:<[^>]+>)?\s*,\s*([A-Za-z0-9_]+)\s*\)\)")
            .map_err(|error| error.to_string())?;
    let mut keys = HashSet::new();
    for segment in [&source[object_start..region_start], &source[region_start..region_end]] {
        for captures in pattern.captures_iter(segment) {
            keys.insert(captures[1].into());
        }
    }
    if keys.is_empty() {
        return Err("object config contains no keys".into());
    }
    Ok(keys)
}

fn cpp_strings(expression: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let bytes = expression.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut escaped = false;
        while index < bytes.len() {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == b'"' {
                values.push(decode_cpp_string(&expression[start..=index])?);
                index += 1;
                break;
            }
            index += 1;
        }
        if index == bytes.len() && bytes.last() != Some(&b'"') {
            return Err("unterminated C++ string".into());
        }
    }
    Ok(values)
}

fn decode_cpp_string(literal: &str) -> Result<String, String> {
    serde_json::from_str(literal)
        .map_err(|error| format!("unsupported C++ string {literal}: {error}"))
}

fn strip_cpp_comments(source: &str) -> Result<String, String> {
    #[derive(Clone, Copy)]
    enum State {
        Code,
        String,
        Character,
        LineComment,
        BlockComment,
    }
    let mut state = State::Code;
    let mut output = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut escaped = false;
    while let Some(character) = chars.next() {
        match state {
            State::Code => match (character, chars.peek().copied()) {
                ('/', Some('/')) => {
                    output.push_str("  ");
                    chars.next();
                    state = State::LineComment;
                }
                ('/', Some('*')) => {
                    output.push_str("  ");
                    chars.next();
                    state = State::BlockComment;
                }
                ('"', _) => {
                    output.push(character);
                    state = State::String;
                    escaped = false;
                }
                ('\'', _) => {
                    output.push(character);
                    state = State::Character;
                    escaped = false;
                }
                _ => output.push(character),
            },
            State::String | State::Character => {
                output.push(character);
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if matches!(state, State::String) && character == '"'
                    || matches!(state, State::Character) && character == '\''
                {
                    state = State::Code;
                }
            }
            State::LineComment => {
                if character == '\n' {
                    output.push('\n');
                    state = State::Code;
                } else {
                    output.push(' ');
                }
            }
            State::BlockComment => {
                if character == '*' && chars.peek() == Some(&'/') {
                    output.push_str("  ");
                    chars.next();
                    state = State::Code;
                } else if character == '\n' {
                    output.push('\n');
                } else {
                    output.push(' ');
                }
            }
        }
    }
    if matches!(state, State::BlockComment | State::String | State::Character) {
        return Err("unterminated C++ token".into());
    }
    Ok(output)
}

fn matching_delimiter(source: &str, start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0_u32;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, character) in source[start..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        if character == '"' {
            quoted = true;
        } else if character == open {
            depth += 1;
        } else if character == close {
            depth -= 1;
            if depth == 0 {
                return Some(start + offset);
            }
        }
    }
    None
}

fn split_top_level(source: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in source.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' | '{' | '[' | '<' => depth += 1,
            ')' | '}' | ']' | '>' => depth -= 1,
            _ if character == delimiter && depth == 0 => {
                parts.push(source[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(source[start..].trim());
    parts
}

fn braced_entries(source: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut index = 0;
    while let Some(relative) = source[index..].find('{') {
        let start = index + relative;
        let Some(end) = matching_delimiter(source, start, '{', '}') else {
            break;
        };
        entries.push(&source[start + 1..end]);
        index = end + 1;
    }
    entries
}

pub fn extract_tab_print_layout(source: &str) -> Result<Vec<Page>, String> {
    let (_, body) =
        source.split_once("void TabPrint::build()").ok_or("TabPrint::build not found")?;
    let (body, _) = body
        .split_once("void TabPrint::reload_config()")
        .ok_or("end of TabPrint::build not found")?;
    let page_pattern = regex::Regex::new(r#"add_options_page\(L\("([^"]+)"\)"#)
        .map_err(|error| error.to_string())?;
    let group_pattern =
        regex::Regex::new(r#"new_optgroup\(L\("([^"]+)"\)"#).map_err(|error| error.to_string())?;
    let direct_option_pattern =
        regex::Regex::new(r#"append_single_option_line\("([A-Za-z0-9_]+)""#)
            .map_err(|error| error.to_string())?;
    let fetched_option_pattern = regex::Regex::new(r#"get_option\("([A-Za-z0-9_]+)"\)"#)
        .map_err(|error| error.to_string())?;
    let mut pages: Vec<Page> = Vec::new();
    for raw_line in body.lines() {
        let line = raw_line.split("//").next().unwrap_or_default();
        if let Some(captures) = page_pattern.captures(line) {
            pages.push(Page { name: captures[1].into(), groups: Vec::new() });
            continue;
        }
        if let Some(captures) = group_pattern.captures(line) {
            let page = pages.last_mut().ok_or("option group appears before page")?;
            page.groups.push(Group { name: captures[1].into(), options: Vec::new() });
            continue;
        }
        let option =
            direct_option_pattern.captures(line).or_else(|| fetched_option_pattern.captures(line));
        if let Some(captures) = option {
            let group = pages
                .last_mut()
                .and_then(|page| page.groups.last_mut())
                .ok_or("option appears before option group")?;
            group.options.push(captures[1].into());
        }
    }
    if pages.is_empty() || pages.iter().all(|page| page.groups.is_empty()) {
        return Err("TabPrint::build contains no process layout".into());
    }
    Ok(pages)
}

pub fn validate(contract: &Contract) -> Result<(), String> {
    validate_identity(contract)?;
    if !contract.capabilities.process_schema
        || contract.capabilities.model_state
        || !contract.capabilities.progress
        || contract.capabilities.cancel
    {
        return Err("capabilities must be process_schema=true, progress=true, model_state=false, cancel=false".into());
    }
    if process_hash(contract).map_err(|error| error.to_string())? != contract.schema_hash {
        return Err("schema_hash mismatch".into());
    }

    let mut option_keys = HashSet::new();
    for option in &contract.process_schema.options {
        let object = option.as_object().ok_or("every option must be an object")?;
        let key = object
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
            .ok_or("every option must have a non-empty key")?;
        let option_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("option metadata missing type: {key}"))?;
        if !matches!(
            option_type,
            "float"
                | "int"
                | "string"
                | "percent"
                | "float_or_percent"
                | "point"
                | "point3"
                | "bool"
                | "enum"
                | "array"
                | "number"
                | "integer"
                | "boolean"
        ) {
            return Err(format!("unsupported option type for {key}: {option_type}"));
        }
        if !option_keys.insert(key) {
            return Err(format!("duplicate option: {key}"));
        }
    }

    let mut placed = HashSet::new();
    let mut page_names = HashSet::new();
    for page in &contract.process_schema.pages {
        if !page_names.insert(page.name.as_str()) {
            return Err(format!("duplicate page: {}", page.name));
        }
        let mut group_names = HashSet::new();
        for group in &page.groups {
            if !group_names.insert(group.name.as_str()) {
                return Err(format!("duplicate group in {}: {}", page.name, group.name));
            }
            for key in &group.options {
                if !option_keys.contains(key.as_str()) {
                    return Err(format!("unknown option: {key}"));
                }
                if !placed.insert(key.as_str()) {
                    return Err(format!("duplicate placed option: {key}"));
                }
            }
        }
    }

    for key in &option_keys {
        if !placed.contains(key) {
            return Err(format!("unplaced option: {key}"));
        }
        if !contract.process_schema.scopes.contains_key(*key) {
            return Err(format!("missing scope: {key}"));
        }
        if !contract.process_schema.samples.contains_key(*key) {
            return Err(format!("missing sample: {key}"));
        }
    }
    if let Some(key) =
        contract.process_schema.scopes.keys().find(|key| !option_keys.contains(key.as_str()))
    {
        return Err(format!("scope for unknown option: {key}"));
    }
    if let Some(key) =
        contract.process_schema.samples.keys().find(|key| !option_keys.contains(key.as_str()))
    {
        return Err(format!("sample for unknown option: {key}"));
    }
    for (key, scope) in &contract.process_schema.scopes {
        let valid_scope = match scope {
            ScopeValue::One(value) => value == "global",
            ScopeValue::Many(values) => {
                values.iter().map(String::as_str).collect::<HashSet<_>>()
                    == HashSet::from(["global", "object"])
                    && values.len() == 2
            }
        };
        if !valid_scope {
            return Err(format!("invalid scope: {key}"));
        }
    }
    Ok(())
}

fn validate_identity(contract: &Contract) -> Result<(), String> {
    if contract.contract_version != CONTRACT_VERSION {
        return Err("unsupported contract_version".into());
    }
    if contract.engine.name != "OrcaSlicer"
        || contract.engine.version != ORCA_VERSION
        || contract.engine.commit != ORCA_COMMIT
    {
        return Err("engine must identify pinned OrcaSlicer 2.4.2 commit".into());
    }
    let digest = &contract.image_identity.digest;
    if digest.len() != 71
        || !digest.starts_with("sha256:")
        || !digest[7..].bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("image_identity.digest must be lowercase sha256".into());
    }
    let scopes = contract.supported_scopes.iter().map(String::as_str).collect::<HashSet<_>>();
    if scopes != HashSet::from(["global", "object"]) {
        return Err("supported_scopes must contain global and object exactly once".into());
    }
    Ok(())
}

pub fn schema() -> Value {
    json!({"contract_version":"1","engine":{"name":"OrcaSlicer","version":"2.4.2","commit":ORCA_COMMIT},"image_identity":{"digest":"sha256:<64 lowercase hex>"},"schema_hash":"<sha256>","capabilities":{"process_schema":false,"model_state":false,"progress":false,"cancel":false},"supported_scopes":["global","object"],"process_schema":{"pages":[],"options":[],"scopes":{},"samples":{}}})
}
