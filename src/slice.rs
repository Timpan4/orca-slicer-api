use crate::{
    config::Config,
    contract::Contract,
    error::AppError,
    metadata,
    model_state::validate_model_state,
    progress::{Progress, ProgressStore},
    storage::{field_bytes, job_dir, safe_filename, write_field},
};
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use serde_json::Value;
use std::{
    collections::HashSet, os::unix::process::CommandExt, process::Stdio, sync::Arc, time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    net::unix::pipe::{OpenOptions as PipeOptions, Receiver as PipeReceiver, Sender as PipeSender},
    process::{Child, Command},
    sync::Semaphore,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub contract: Arc<Contract>,
    pub profiles: Arc<Value>,
    pub progress: ProgressStore,
    pub slots: Arc<Semaphore>,
}

const MAX_MODEL: usize = 512 * 1024 * 1024;
const MAX_PROFILE: usize = 32 * 1024 * 1024;
const SLICE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

struct ProcessGroup(Option<rustix::process::Pid>);

impl ProcessGroup {
    fn terminate(&mut self) {
        if let Some(pid) = self.0.take() {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_process(command: &mut Command) -> Result<(Child, ProcessGroup), AppError> {
    command.as_std_mut().process_group(0);
    command.kill_on_drop(true);
    let child = command.spawn().map_err(|error| AppError::Execution(error.to_string()))?;
    let raw_pid = child.id().ok_or(AppError::Internal)?;
    let pid = i32::try_from(raw_pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or(AppError::Internal)?;
    Ok((child, ProcessGroup(Some(pid))))
}

async fn create_progress_pipe(
    path: &std::path::Path,
) -> Result<(PipeReceiver, PipeSender), AppError> {
    let status = Command::new("/usr/bin/mkfifo")
        .args(["-m", "600"])
        .arg(path)
        .status()
        .await
        .map_err(|error| AppError::Execution(error.to_string()))?;
    if !status.success() {
        return Err(AppError::Execution("failed to create slicer progress pipe".into()));
    }
    let receiver = PipeOptions::new()
        .open_receiver(path)
        .map_err(|error| AppError::Execution(error.to_string()))?;
    let sender = PipeOptions::new()
        .open_sender(path)
        .map_err(|error| AppError::Execution(error.to_string()))?;
    Ok((receiver, sender))
}

pub async fn slice(State(s): State<AppState>, mut mp: Multipart) -> Result<Response, AppError> {
    let request_id = Uuid::new_v4().to_string();
    let dir = job_dir(&s.config.data_dir, &request_id).await?;
    let result = slice_job(&s, &mut mp, &request_id, &dir).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
    result
}

async fn slice_job(
    s: &AppState,
    mp: &mut Multipart,
    request_id: &str,
    dir: &std::path::Path,
) -> Result<Response, AppError> {
    let _permit = s
        .slots
        .try_acquire()
        .map_err(|_| AppError::Unavailable("slicer capacity exhausted".into()))?;
    let mut seen = HashSet::new();
    let mut model: Option<std::path::PathBuf> = None;
    let mut printer = None;
    let mut preset = None;
    let mut filaments = Vec::new();
    let mut plate = None;
    let mut export = false;
    let mut arrange = false;
    let mut schema_hash = None;
    let mut supplied_request_id = None;
    let mut model_state = None;
    while let Some(field) = mp.next_field().await.map_err(|e| AppError::Bad(e.to_string()))? {
        let name = field
            .name()
            .ok_or_else(|| AppError::Bad("multipart field name required".into()))?
            .to_string();
        if name != "filamentProfile" && !seen.insert(name.clone()) {
            return Err(AppError::Bad("duplicate multipart field".into()));
        }
        match name.as_str() {
            "file" => {
                if model.is_some() {
                    return Err(AppError::Bad("duplicate model field".into()));
                }
                let path = dir.join(safe_filename(field.file_name(), "model"));
                write_field(field, &path, MAX_MODEL).await.map_err(AppError::Bad)?;
                model = Some(path);
            }
            "printerProfile" | "presetProfile" => {
                let expected = if name == "printerProfile" { "machine" } else { "process" };
                let bytes = profile_bytes(field, expected).await?;
                let path = dir.join(&name);
                tokio::fs::write(&path, bytes).await?;
                if name == "printerProfile" {
                    printer = Some(path);
                } else {
                    preset = Some(path);
                }
            }
            "filamentProfile" => {
                if filaments.len() == 16 {
                    return Err(AppError::Bad("maximum 16 filament profiles".into()));
                }
                let bytes = profile_bytes(field, "filament").await?;
                let path = dir.join(format!("filament_{}", filaments.len()));
                tokio::fs::write(&path, bytes).await?;
                filaments.push(path);
            }
            "plate" => {
                plate = Some(parse_u32(field_bytes(field, 32).await.map_err(AppError::Bad)?)?);
            }
            "exportType" => {
                let x = field_bytes(field, 32).await.map_err(AppError::Bad)?;
                if x != b"3mf" {
                    return Err(AppError::Bad("exportType must be 3mf".into()));
                }
                export = true;
            }
            "arrange" => {
                arrange = parse_bool(&field_bytes(field, 32).await.map_err(AppError::Bad)?)?;
            }
            "schemaHash" => {
                let x = String::from_utf8(field_bytes(field, 128).await.map_err(AppError::Bad)?)
                    .map_err(|_| AppError::Bad("invalid schemaHash".into()))?;
                if x.len() != 64
                    || !x.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return Err(AppError::Bad("schemaHash must be lowercase SHA-256".into()));
                }
                schema_hash = Some(x);
            }
            "requestId" => {
                let value =
                    String::from_utf8(field_bytes(field, 128).await.map_err(AppError::Bad)?)
                        .map_err(|_| AppError::Bad("invalid requestId".into()))?;
                if value.is_empty()
                    || value.len() > 128
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
                {
                    return Err(AppError::Bad("requestId contains invalid characters".into()));
                }
                supplied_request_id = Some(value);
            }
            "modelState" => {
                let x = field_bytes(field, 256 * 1024).await.map_err(AppError::Bad)?;
                let v: Value = serde_json::from_slice(&x)?;
                validate_model_state(&v, &s.contract)?;
                model_state = Some(v);
            }
            _ => return Err(AppError::Bad(format!("unknown multipart field: {name}"))),
        }
    }
    if model.is_none() {
        return Err(AppError::Bad("file is required".into()));
    }
    if printer.is_some() != preset.is_some() || (!filaments.is_empty() && printer.is_none()) {
        return Err(AppError::Bad(
            "printerProfile, presetProfile, and filamentProfile must form a complete profile set"
                .into(),
        ));
    }
    if let Some(hash) = &schema_hash
        && hash != &s.contract.schema_hash
    {
        return Err(AppError::Conflict(format!(
            "declared schema hash {} does not match {hash}",
            s.contract.schema_hash
        )));
    }
    if model_state.is_some() && schema_hash.is_none() {
        return Err(AppError::Bad("schemaHash is required with modelState".into()));
    }
    if model_state.is_some() && !s.contract.capabilities.model_state {
        return Err(AppError::Bad("modelState is not supported by this image".into()));
    }
    if model_state.is_some() && s.config.bridge_path.is_none() {
        return Err(AppError::Bad("modelState requires configured bridge".into()));
    }
    let progress_id = supplied_request_id.as_deref().unwrap_or(request_id);
    if !s
        .progress
        .reserve(
            progress_id,
            Progress {
                stage: "running".into(),
                total_percent: Some(0.0),
                plate_percent: None,
                plate_index: plate,
                plate_count: None,
                updated_at: 0,
            },
        )
        .await
    {
        return Err(AppError::Conflict("requestId is already in use".into()));
    }
    let input = model.unwrap();
    if let (Some(bridge), Some(state)) = (&s.config.bridge_path, model_state) {
        let state_path = dir.join("model-state.json");
        tokio::fs::write(&state_path, serde_json::to_vec(&state)?).await?;
        let output = dir.join("prepared-model.3mf");
        let mut command = Command::new(bridge);
        command
            .args(["prepare-model", "--input"])
            .arg(&input)
            .args(["--model-state"])
            .arg(&state_path)
            .args(["--output"])
            .arg(&output);
        let (mut child, mut group) = spawn_process(&mut command)?;
        let status = match tokio::time::timeout(SLICE_TIMEOUT, child.wait()).await {
            Ok(status) => status.map_err(|error| AppError::Execution(error.to_string()))?,
            Err(_) => {
                group.terminate();
                let _ = child.wait().await;
                return Err(AppError::Execution("bridge exceeded 30 minute timeout".into()));
            }
        };
        group.terminate();
        if !status.success() {
            return Err(AppError::Bad("bridge prepare-model failed".into()));
        }
        return run_cli(
            s,
            progress_id,
            dir,
            &output,
            printer.as_deref(),
            preset.as_deref(),
            &filaments,
            plate,
            export,
            arrange,
        )
        .await;
    }
    run_cli(
        s,
        progress_id,
        dir,
        &input,
        printer.as_deref(),
        preset.as_deref(),
        &filaments,
        plate,
        export,
        arrange,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_cli(
    s: &AppState,
    id: &str,
    dir: &std::path::Path,
    input: &std::path::Path,
    printer: Option<&std::path::Path>,
    preset: Option<&std::path::Path>,
    filaments: &[std::path::PathBuf],
    plate: Option<u32>,
    export: bool,
    arrange: bool,
) -> Result<Response, AppError> {
    s.progress
        .set(
            id,
            Progress {
                stage: "running".into(),
                total_percent: Some(0.0),
                plate_percent: None,
                plate_index: plate,
                plate_count: None,
                updated_at: 0,
            },
        )
        .await;
    let result =
        execute_cli(s, id, dir, input, printer, preset, filaments, plate, export, arrange).await;
    if result.is_err() {
        s.progress
            .set(
                id,
                Progress {
                    stage: "failed".into(),
                    total_percent: None,
                    plate_percent: None,
                    plate_index: plate,
                    plate_count: None,
                    updated_at: 0,
                },
            )
            .await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn execute_cli(
    s: &AppState,
    id: &str,
    dir: &std::path::Path,
    input: &std::path::Path,
    printer: Option<&std::path::Path>,
    preset: Option<&std::path::Path>,
    filaments: &[std::path::PathBuf],
    plate: Option<u32>,
    export: bool,
    arrange: bool,
) -> Result<Response, AppError> {
    let output_dir = dir.join("output");
    tokio::fs::create_dir(&output_dir).await?;
    let progress_path = dir.join("progress.pipe");
    let (progress_reader, progress_writer) = create_progress_pipe(&progress_path).await?;
    let mut cmd = Command::new(&s.config.cli_path);
    cmd.arg("--pipe").arg(&progress_path);
    cmd.arg("--slice").arg(plate.unwrap_or(1).to_string());
    if arrange {
        cmd.arg("--arrange").arg("1");
    }
    if export {
        cmd.arg("--export-3mf").arg("result.3mf");
    }
    if let Some(p) = printer {
        cmd.arg("--load-settings").arg(format!(
            "{};{}",
            p.to_string_lossy(),
            preset.unwrap().to_string_lossy()
        ));
    }
    if !filaments.is_empty() {
        cmd.arg("--load-filaments")
            .arg(filaments.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join(";"));
    }
    cmd.arg("--allow-newer-file")
        .arg("--outputdir")
        .arg(&output_dir)
        .arg(input)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, mut group) = spawn_process(&mut cmd)?;
    let stdout = child.stdout.take().ok_or(AppError::Internal)?;
    let stderr = child.stderr.take().ok_or(AppError::Internal)?;
    let progress = s.progress.clone();
    let progress_id = id.to_owned();
    let progress_task = tokio::spawn(async move {
        let mut lines = BufReader::new(progress_reader).lines();
        while let Some(line) = lines.next_line().await? {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                progress.set(&progress_id, progress_from_json(&value, plate)).await;
            }
        }
        Ok::<(), std::io::Error>(())
    });
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        BufReader::new(stdout).read_to_end(&mut bytes).await?;
        Ok::<(), std::io::Error>(())
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        BufReader::new(stderr).read_to_end(&mut bytes).await?;
        Ok::<Vec<u8>, std::io::Error>(bytes)
    });
    let status = match tokio::time::timeout(SLICE_TIMEOUT, child.wait()).await {
        Ok(status) => status.map_err(|error| AppError::Execution(error.to_string()))?,
        Err(_) => {
            group.terminate();
            let _ = child.wait().await;
            drop(progress_writer);
            progress_task.abort();
            stdout_task.abort();
            stderr_task.abort();
            return Err(AppError::Execution("slicer exceeded 30 minute timeout".into()));
        }
    };
    group.terminate();
    drop(progress_writer);
    progress_task.abort();
    let _ = progress_task.await;
    stdout_task
        .await
        .map_err(|error| AppError::Execution(error.to_string()))?
        .map_err(|error| AppError::Execution(error.to_string()))?;
    let stderr = stderr_task
        .await
        .map_err(|error| AppError::Execution(error.to_string()))?
        .map_err(|error| AppError::Execution(error.to_string()))?;
    if !status.success() {
        return Err(AppError::Execution(format!(
            "slicer exited unsuccessfully: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    let mut entries = tokio::fs::read_dir(&output_dir).await?;
    let mut found = None;
    let expected_extension = if export { "3mf" } else { "gcode" };
    while let Some(e) = entries.next_entry().await? {
        let p = e.path();
        if p.extension().and_then(|extension| extension.to_str()) == Some(expected_extension) {
            if found.is_some() {
                return Err(AppError::Execution("slicer produced multiple outputs".into()));
            }
            found = Some(p);
        }
    }
    let output = found.ok_or_else(|| AppError::Execution("slicer produced no output".into()))?;
    let bytes = tokio::fs::read(&output).await?;
    let output_metadata = metadata::from_artifact(
        &bytes,
        output.extension().and_then(|extension| extension.to_str()),
        plate,
    )
    .map_err(AppError::Execution)?;
    s.progress
        .set(
            id,
            Progress {
                stage: "completed".into(),
                total_percent: Some(100.0),
                plate_percent: Some(100.0),
                plate_index: plate,
                plate_count: None,
                updated_at: 0,
            },
        )
        .await;
    let mut h = HeaderMap::new();
    h.insert("x-request-id", HeaderValue::from_str(id).unwrap());
    h.insert("content-type", HeaderValue::from_static("application/octet-stream"));
    h.insert(
        "x-print-time-seconds",
        HeaderValue::from_str(&output_metadata.print_time_seconds.to_string()).unwrap(),
    );
    h.insert(
        "x-filament-used-g",
        HeaderValue::from_str(&output_metadata.filament_used_g.to_string()).unwrap(),
    );
    h.insert(
        "x-filament-used-mm",
        HeaderValue::from_str(&output_metadata.filament_used_mm.to_string()).unwrap(),
    );
    let response = Response::new(Body::from(bytes));
    let _ = tokio::fs::remove_dir_all(dir).await;
    Ok(response.with_headers(h))
}

fn progress_from_json(value: &Value, default_plate: Option<u32>) -> Progress {
    Progress {
        stage: value.get("stage").and_then(Value::as_str).unwrap_or("running").into(),
        total_percent: value
            .get("total_percent")
            .or_else(|| value.get("totalPercent"))
            .and_then(Value::as_f64)
            .map(|number| number as f32),
        plate_percent: value
            .get("plate_percent")
            .or_else(|| value.get("platePercent"))
            .and_then(Value::as_f64)
            .map(|number| number as f32),
        plate_index: value
            .get("plate_index")
            .or_else(|| value.get("plateIndex"))
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .or(default_plate),
        plate_count: value
            .get("plate_count")
            .or_else(|| value.get("plateCount"))
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok()),
        updated_at: 0,
    }
}

async fn profile_bytes(
    field: axum::extract::multipart::Field<'_>,
    expected: &str,
) -> Result<Vec<u8>, AppError> {
    let bytes = field_bytes(field, MAX_PROFILE).await.map_err(AppError::Bad)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Bad(format!("{expected} profile must be a JSON object")))?;
    let object = value
        .as_object()
        .ok_or_else(|| AppError::Bad(format!("{expected} profile must be a JSON object")))?;
    if object.get("type").and_then(Value::as_str) != Some(expected)
        || object.get("name").and_then(Value::as_str).is_none_or(str::is_empty)
        || object.get("setting_id").and_then(Value::as_str).is_none_or(str::is_empty)
    {
        return Err(AppError::Bad(format!(
            "{expected} profile requires type, name, and setting_id"
        )));
    }
    if let Some(inherits) = object.get("inherits")
        && inherits.as_str().is_none_or(str::is_empty)
    {
        return Err(AppError::Bad(format!(
            "{expected} profile inherits must be a non-empty string"
        )));
    }
    Ok(bytes)
}

fn parse_u32(b: Vec<u8>) -> Result<u32, AppError> {
    let s = String::from_utf8(b).map_err(|_| AppError::Bad("invalid plate".into()))?;
    let n = s.parse().map_err(|_| AppError::Bad("plate must be non-negative integer".into()))?;
    Ok(n)
}
fn parse_bool(b: &[u8]) -> Result<bool, AppError> {
    match b {
        b"true" | b"1" => Ok(true),
        b"false" | b"0" => Ok(false),
        _ => Err(AppError::Bad("invalid boolean".into())),
    }
}
trait ResponseHeaders {
    fn with_headers(self, h: HeaderMap) -> Response;
}
impl ResponseHeaders for Response {
    fn with_headers(mut self, h: HeaderMap) -> Response {
        *self.headers_mut() = h;
        self
    }
}
pub async fn progress(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::Json<Progress>, AppError> {
    s.progress.get(&id).await.map(axum::Json).ok_or(AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn process_group_guard_kills_descendants() {
        let root = std::env::temp_dir().join(format!("orca-process-group-{}", std::process::id()));
        let ready = root.join("ready");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut command = Command::new("/bin/sh");
        command.env("READY", &ready).args(["-c", "sleep 30 & echo ready > \"$READY\"; wait"]);
        let (mut child, group) = spawn_process(&mut command).unwrap();
        let process_group = group.0.unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready.exists(), "child process did not spawn its descendant");

        drop(group);
        tokio::time::timeout(Duration::from_secs(2), child.wait()).await.unwrap().unwrap();
        for _ in 0..100 {
            if rustix::process::test_kill_process_group(process_group).is_err() {
                std::fs::remove_dir_all(root).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("descendant survived process-group termination");
    }
}
