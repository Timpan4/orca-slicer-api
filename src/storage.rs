use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::{fs, io::AsyncWriteExt};

pub async fn reset_jobs(base: &Path) -> io::Result<()> {
    let jobs = base.join("jobs");
    match fs::remove_dir_all(&jobs).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir_all(jobs).await
}

pub async fn job_dir(base: &Path, id: &str) -> io::Result<PathBuf> {
    let p = base.join("jobs").join(id);
    fs::create_dir_all(&p).await?;
    Ok(p)
}

pub fn safe_filename(name: Option<&str>, fallback: &str) -> String {
    let name = name.unwrap_or(fallback).replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or(fallback);
    let name = name.trim_matches('.');
    if name.is_empty() || name == "." || name == ".." { fallback.into() } else { name.into() }
}

pub async fn write_field(
    mut field: axum::extract::multipart::Field<'_>,
    path: &Path,
    max: usize,
) -> Result<(), String> {
    let mut file = fs::File::create(path).await.map_err(|e| e.to_string())?;
    let mut total = 0;
    while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
        total += chunk.len();
        if total > max {
            return Err("multipart field too large".into());
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())
}

pub async fn field_bytes(
    mut field: axum::extract::multipart::Field<'_>,
    max: usize,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|e| e.to_string())? {
        if out.len() + chunk.len() > max {
            return Err("multipart field too large".into());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
