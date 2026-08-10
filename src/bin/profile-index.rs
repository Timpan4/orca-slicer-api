use orca_slicer_api::profiles::load_profile_catalog;
use std::{env, fs, path::PathBuf, process::exit};

fn argument(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2).find(|pair| pair[0] == name).map(|pair| PathBuf::from(&pair[1]))
}

fn run() -> Result<(), String> {
    let args = env::args().collect::<Vec<_>>();
    let root = argument(&args, "--profiles")
        .ok_or("usage: profile-index --profiles DIRECTORY --output FILE")?;
    let output = argument(&args, "--output")
        .ok_or("usage: profile-index --profiles DIRECTORY --output FILE")?;
    let catalog = load_profile_catalog(&root)?;
    let index = catalog.decorate_index(catalog.base_index())?;
    fs::write(output, serde_json::to_vec(&index).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("profile index failed: {error}");
        exit(1);
    }
}
