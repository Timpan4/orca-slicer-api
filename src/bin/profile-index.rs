use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::exit,
};

#[derive(Serialize)]
struct ProfileEntry {
    name: String,
    base_id: String,
}

fn argument(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2).find(|pair| pair[0] == name).map(|pair| PathBuf::from(&pair[1]))
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file()
            && entry.path().extension().is_some_and(|extension| extension == "json")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args = env::args().collect::<Vec<_>>();
    let root = argument(&args, "--profiles")
        .ok_or("usage: profile-index --profiles DIRECTORY --output FILE")?;
    let output = argument(&args, "--output")
        .ok_or("usage: profile-index --profiles DIRECTORY --output FILE")?;
    let mut files = Vec::new();
    collect_files(&root, &mut files)?;
    let mut index = BTreeMap::from([
        ("printer".to_owned(), Vec::<ProfileEntry>::new()),
        ("process".to_owned(), Vec::new()),
        ("filament".to_owned(), Vec::new()),
    ]);
    let mut ids = HashSet::new();
    for path in files {
        let profile: Value =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        let slot = match profile.get("type").and_then(Value::as_str) {
            Some("machine") => "printer",
            Some("process") => "process",
            Some("filament") => "filament",
            _ => continue,
        };
        if profile.get("instantiation").and_then(Value::as_str) == Some("false") {
            continue;
        }
        let name = profile
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{}: profile name required", path.display()))?;
        let relative = path.strip_prefix(&root).map_err(|error| error.to_string())?;
        let base_id = profile
            .get("setting_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map_or_else(|| relative.to_string_lossy().replace('\\', "/"), str::to_owned);
        if !ids.insert((slot, base_id.clone())) {
            return Err(format!("duplicate {slot} profile ID: {base_id}"));
        }
        index.get_mut(slot).unwrap().push(ProfileEntry { name: name.into(), base_id });
    }
    for profiles in index.values_mut() {
        profiles.sort_by(|left, right| {
            left.name.cmp(&right.name).then(left.base_id.cmp(&right.base_id))
        });
    }
    fs::write(output, serde_json::to_vec(&index).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("profile index failed: {error}");
        exit(1);
    }
}
