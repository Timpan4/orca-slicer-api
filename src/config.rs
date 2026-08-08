use std::{env, path::PathBuf};

#[derive(Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub cli_path: String,
    pub bridge_path: Option<String>,
    pub image_digest: String,
    pub schema_path: PathBuf,
    pub profiles_path: Option<PathBuf>,
    pub profile_source_path: Option<PathBuf>,
    pub max_concurrency: usize,
}
impl Config {
    pub fn from_env() -> Result<Self, String> {
        let image_digest = env::var("ORCA_IMAGE_DIGEST")
            .map_err(|_| "ORCA_IMAGE_DIGEST is required".to_string())?;
        if image_digest.len() != 71
            || !image_digest.starts_with("sha256:")
            || !image_digest[7..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("ORCA_IMAGE_DIGEST must be lowercase sha256".into());
        }
        let schema_path = env::var("ORCA_SCHEMA_PATH")
            .map_err(|_| "ORCA_SCHEMA_PATH is required".to_string())
            .map(PathBuf::from)?;
        let max_concurrency = env::var("ORCA_MAX_CONCURRENCY")
            .ok()
            .map_or(Ok(2), |v| {
                v.parse::<usize>().map_err(|_| "invalid ORCA_MAX_CONCURRENCY".to_string())
            })
            .and_then(|n| {
                if n > 0 { Ok(n) } else { Err("ORCA_MAX_CONCURRENCY must be positive".into()) }
            })?;
        Ok(Self {
            data_dir: env::var_os("ORCA_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/app/data")),
            cli_path: env::var("ORCA_CLI_PATH").unwrap_or_else(|_| "orca-slicer".into()),
            bridge_path: env::var("ORCA_BRIDGE_PATH").ok(),
            image_digest,
            schema_path,
            profiles_path: env::var("ORCA_PROFILES_PATH").ok().map(PathBuf::from),
            profile_source_path: env::var("ORCA_PROFILE_SOURCE_PATH").ok().map(PathBuf::from),
            max_concurrency,
        })
    }
}
