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
    let (bundled_profiles, profile_catalog) =
        match (config.profiles_path.as_ref(), config.profile_source_path.as_ref()) {
            (Some(index_path), Some(source_path)) => {
                let catalog =
                    profiles::load_profile_catalog(source_path).map_err(std::io::Error::other)?;
                let index =
                    profiles::load_profile_index(index_path).map_err(std::io::Error::other)?;
                (
                    profiles::ProfileCatalog::decorate_index(&catalog, index)
                        .map_err(std::io::Error::other)?,
                    catalog,
                )
            }
            (None, None) => (
                serde_json::json!({"printer": [], "process": [], "filament": []}),
                Default::default(),
            ),
            _ => {
                return Err(std::io::Error::other(
                    "ORCA_PROFILES_PATH and ORCA_PROFILE_SOURCE_PATH must be configured together",
                )
                .into());
            }
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
        profile_catalog: Arc::new(profile_catalog),
        config,
    };
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
