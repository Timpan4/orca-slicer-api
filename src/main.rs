use orca_slicer_api::{
    api, config::Config, contract::parse_contract, profiles, progress, slice::AppState, storage,
};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Semaphore};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let config = Config::from_env().map_err(std::io::Error::other)?;
    let mut contract =
        parse_contract(&std::fs::read(&config.schema_path)?).map_err(std::io::Error::other)?;
    contract.image_identity.digest.clone_from(&config.image_digest);
    let bundled_profiles = if let Some(path) = &config.profiles_path {
        profiles::load_profile_index(path).map_err(std::io::Error::other)?
    } else {
        serde_json::json!({"printer": [], "process": [], "filament": []})
    };
    tokio::fs::create_dir_all(&config.data_dir).await?;
    storage::reset_jobs(&config.data_dir).await?;
    for directory in ["tmp", "cache", "config"] {
        tokio::fs::create_dir_all(config.data_dir.join(directory)).await?;
    }
    let state = AppState {
        slots: Arc::new(Semaphore::new(config.max_concurrency)),
        progress: progress::ProgressStore::new(),
        contract: Arc::new(contract),
        profiles: Arc::new(bundled_profiles),
        config,
    };
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
