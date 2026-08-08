use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use orca_slicer_api::{
    api,
    config::Config,
    contract::{
        Capabilities, Contract, Engine, Group, ImageIdentity, Page, ProcessSchema, ScopeValue,
        process_hash,
    },
    progress::ProgressStore,
    slice::AppState,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn contract() -> Contract {
    let mut contract = Contract {
        contract_version: "1".into(),
        engine: Engine {
            name: "OrcaSlicer".into(),
            version: "2.4.2".into(),
            commit: "8500fcdccaa10b5099ac20d252af3a7c560046f1".into(),
        },
        image_identity: ImageIdentity { digest: format!("sha256:{}", "a".repeat(64)) },
        schema_hash: String::new(),
        capabilities: Capabilities {
            process_schema: true,
            model_state: false,
            progress: true,
            cancel: false,
        },
        supported_scopes: vec!["global".into(), "object".into()],
        process_schema: ProcessSchema {
            pages: vec![Page {
                name: "Quality".into(),
                groups: vec![Group { name: "Layers".into(), options: vec!["layer_height".into()] }],
            }],
            options: vec![json!({"key":"layer_height","type":"number"})],
            scopes: BTreeMap::from([("layer_height".into(), ScopeValue::One("global".into()))]),
            samples: BTreeMap::from([("layer_height".into(), json!(0.2))]),
        },
    };
    contract.schema_hash = process_hash(&contract).unwrap();
    contract
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("orca-api-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn fake_cli(root: &Path) -> PathBuf {
    let path = root.join("fake-orca.sh");
    let args_path = root.join("args.txt");
    let cwd_path = root.join("cwd.txt");
    let cwd_writable_path = root.join("cwd-writable.txt");
    let archive_path = root.join("fixture.3mf");
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(
            "Metadata/plate_0.gcode",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
    archive
        .write_all(b"; estimated printing time (normal mode) = 10m 56s\n; filament used [mm] = 302.50\n; total filament used [g] = 0.94\nfixture-gcode")
        .unwrap();
    fs::write(&archive_path, archive.finish().unwrap().into_inner()).unwrap();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\npwd > '{}'\ntouch \"$PWD/cwd-writable\" || exit 1\nprintf writable > '{}'\nprintf '%s\\n' \"$@\" > '{}'\nexport_3mf=0\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--outputdir' ]; then shift; output=$1; fi\n  if [ \"$1\" = '--export-3mf' ]; then export_3mf=1; fi\n  shift\ndone\nif [ \"$export_3mf\" = 1 ]; then\n  cp '{}' \"$output/result.3mf\"\nelse\n  printf '; estimated printing time (normal mode) = 10m 56s\n; filament used [mm] = 302.50\n; total filament used [g] = 0.94\nfixture-gcode' > \"$output/result.gcode\"\nfi\n",
            cwd_path.display(),
            cwd_writable_path.display(),
            args_path.display(),
            archive_path.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn slow_cli(root: &Path) -> PathBuf {
    let path = root.join("slow-orca.sh");
    fs::write(
        &path,
        "#!/bin/sh\npipe=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--pipe' ]; then shift; pipe=$1; fi\n  if [ \"$1\" = '--outputdir' ]; then shift; output=$1; fi\n  shift\ndone\nprintf '%s\\n' '{\"plate_index\":1,\"plate_count\":1,\"plate_percent\":37,\"total_percent\":37,\"message\":\"Slicing\"}' > \"$pipe\"\nsleep 1\nprintf '; estimated printing time (normal mode) = 10m 56s\n; filament used [mm] = 302.50\n; total filament used [g] = 0.94\nfixture-gcode' > \"$output/result.gcode\"\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn descendant_cli(root: &Path) -> PathBuf {
    let path = root.join("descendant-orca.sh");
    let group_path = root.join("process-group.txt");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nsleep 30 >/dev/null 2>&1 &\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--outputdir' ]; then shift; output=$1; fi\n  shift\ndone\nprintf '; estimated printing time (normal mode) = 1s\n; filament used [mm] = 1.0\n; total filament used [g] = 1.0\nfixture-gcode' > \"$output/result.gcode\"\n",
            group_path.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn state(root: &Path) -> AppState {
    let contract = contract();
    AppState {
        config: Config {
            data_dir: root.join("data"),
            cli_path: fake_cli(root).to_string_lossy().into_owned(),
            bridge_path: None,
            image_digest: contract.image_identity.digest.clone(),
            schema_path: PathBuf::from("unused"),
            profiles_path: None,
            max_concurrency: 1,
        },
        contract: Arc::new(contract),
        profiles: Arc::new(json!({"printer":[],"process":[],"filament":[]})),
        progress: ProgressStore::new(),
        slots: Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

fn multipart(parts: &[(&str, Option<&str>, &str)]) -> (String, Vec<u8>) {
    let boundary = "orca-boundary";
    let mut body = Vec::new();
    for (name, filename, value) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            match filename {
                Some(filename) => format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                ),
                None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
            }
            .as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn slice_uses_layercove_wire_and_returns_artifact_headers() {
    let root = test_dir("slice-wire");
    let state = state(&root);
    let schema_hash = state.contract.schema_hash.clone();
    let app = api::router(state);
    let (content_type, body) = multipart(&[
        ("file", Some("cube.stl"), "solid cube"),
        (
            "printerProfile",
            Some("printer.json"),
            r#"{"type":"machine","name":"Printer","setting_id":"printer"}"#,
        ),
        (
            "presetProfile",
            Some("preset.json"),
            r#"{"type":"process","name":"Preset","setting_id":"preset","inherits":"Base Process"}"#,
        ),
        (
            "filamentProfile",
            Some("filament-1.json"),
            r#"{"type":"filament","name":"Filament 1","setting_id":"filament-1","inherits":"Base Filament"}"#,
        ),
        (
            "filamentProfile",
            Some("filament-2.json"),
            r#"{"type":"filament","name":"Filament 2","setting_id":"filament-2","inherits":"Base Filament"}"#,
        ),
        ("plate", None, "0"),
        ("exportType", None, "3mf"),
        ("arrange", None, "true"),
        ("schemaHash", None, &schema_hash),
        ("requestId", None, "job-1"),
    ]);
    let response = app
        .clone()
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-request-id"], "job-1");
    assert_eq!(response.headers()["x-print-time-seconds"], "656");
    assert_eq!(response.headers()["x-filament-used-g"], "0.94");
    assert_eq!(response.headers()["x-filament-used-mm"], "302.5");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.starts_with(b"PK"));

    let args = fs::read_to_string(root.join("args.txt")).unwrap();
    assert!(args.contains("--pipe\n"));
    assert!(args.contains("--slice\n0\n"));
    assert!(args.contains("--arrange\n1\n"));
    assert!(args.contains("--export-3mf\nresult.3mf\n"));
    assert!(args.contains("--load-settings\n"));
    assert!(args.contains("--load-filaments\n"));
    assert!(args.contains("filament_0;"));
    assert!(args.contains("filament_1"));
    assert!(args.contains("--allow-newer-file\n--outputdir\n"));

    let progress = app
        .oneshot(Request::get("/slice/progress/job-1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(progress.status(), StatusCode::OK);
    let progress = progress.into_body().collect().await.unwrap().to_bytes();
    let progress: serde_json::Value = serde_json::from_slice(&progress).unwrap();
    assert_eq!(progress["stage"], "completed");
    assert_eq!(progress["total_percent"], 100.0);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn slice_rejects_invalid_profile_shapes_before_running_orca() {
    let root = test_dir("invalid-profiles");
    let app = api::router(state(&root));
    for (field, profile, expected) in [
        ("printerProfile", "[]", "machine profile must be a JSON object"),
        (
            "printerProfile",
            r#"{"type":"process","name":"Printer","setting_id":"printer"}"#,
            "machine profile requires type, name, and setting_id",
        ),
        (
            "filamentProfile",
            r#"{"type":"filament","name":"Filament","setting_id":"filament","inherits":null}"#,
            "filament profile inherits must be a non-empty string",
        ),
    ] {
        let (content_type, body) = multipart(&[
            ("file", Some("cube.stl"), "solid cube"),
            (field, Some("profile.json"), profile),
        ]);
        let response = app
            .clone()
            .oneshot(
                Request::post("/slice")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["details"], expected);
    }
    assert!(!root.join("args.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn duplicate_active_request_id_is_rejected_before_running_orca() {
    let root = test_dir("duplicate-request-id");
    let state = state(&root);
    assert!(
        state
            .progress
            .reserve(
                "shared-job",
                orca_slicer_api::progress::Progress {
                    stage: "running".into(),
                    total_percent: Some(0.0),
                    plate_percent: None,
                    plate_index: None,
                    plate_count: None,
                    updated_at: 0,
                },
            )
            .await
    );
    let app = api::router(state);
    let (content_type, body) =
        multipart(&[("file", Some("cube.stl"), "solid cube"), ("requestId", None, "shared-job")]);
    let response = app
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["details"], "requestId is already in use");
    assert!(!root.join("args.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn slice_runs_orca_in_private_job_directory() {
    let root = test_dir("slice-cwd");
    let app = api::router(state(&root));
    let (content_type, body) = multipart(&[("file", Some("cube.stl"), "solid cube")]);

    let response = app
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let cwd = PathBuf::from(fs::read_to_string(root.join("cwd.txt")).unwrap().trim());
    let jobs = fs::canonicalize(root.join("data/jobs")).unwrap();
    assert_eq!(cwd.parent(), Some(jobs.as_path()));
    assert!(root.join("cwd-writable.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn successful_cli_exit_kills_remaining_process_group() {
    let root = test_dir("successful-descendant-cleanup");
    let mut app_state = state(&root);
    app_state.config.cli_path = descendant_cli(&root).to_string_lossy().into_owned();
    let app = api::router(app_state);
    let (content_type, body) = multipart(&[("file", Some("cube.stl"), "solid cube")]);
    let response = app
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response_body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&response_body));

    let raw_group: i32 =
        fs::read_to_string(root.join("process-group.txt")).unwrap().trim().parse().unwrap();
    let group = rustix::process::Pid::from_raw(raw_group).unwrap();
    for _ in 0..100 {
        if rustix::process::test_kill_process_group(group).is_err() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("descendant survived successful slicer exit");
}

#[tokio::test]
async fn progress_updates_before_slice_finishes() {
    let root = test_dir("live-progress");
    let mut state = state(&root);
    state.config.cli_path = slow_cli(&root).to_string_lossy().into_owned();
    let app = api::router(state);
    let (content_type, body) =
        multipart(&[("file", Some("cube.stl"), "solid cube"), ("requestId", None, "live-job")]);
    let slice_app = app.clone();
    let slice = tokio::spawn(async move {
        slice_app
            .oneshot(
                Request::post("/slice")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    });

    let mut observed = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let response = app
            .clone()
            .oneshot(Request::get("/slice/progress/live-job").body(Body::empty()).unwrap())
            .await
            .unwrap();
        if response.status() == StatusCode::OK {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let progress: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if progress["stage"] == "running"
                && progress["total_percent"] == 37.0
                && progress["plate_percent"] == 37.0
                && progress["plate_index"] == 1
                && progress["plate_count"] == 1
            {
                observed = true;
                break;
            }
        }
    }

    assert!(observed, "progress event was not visible while slicer ran");
    assert_eq!(slice.await.unwrap().status(), StatusCode::OK);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn slice_rejects_work_when_capacity_is_exhausted() {
    let root = test_dir("capacity");
    let state = state(&root);
    let _permit = state.slots.clone().acquire_owned().await.unwrap();
    let app = api::router(state);
    let (content_type, body) = multipart(&[("file", Some("cube.stl"), "solid cube")]);
    let response = app
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!root.join("args.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn slice_rejects_stale_schema_before_running_orca() {
    let root = test_dir("stale-schema");
    let app = api::router(state(&root));
    let (content_type, body) = multipart(&[
        ("file", Some("cube.stl"), "solid cube"),
        ("schemaHash", None, &"0".repeat(64)),
    ]);
    let response = app
        .oneshot(
            Request::post("/slice")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!root.join("args.txt").exists());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["message"], "Slicer schema mismatch");
    fs::remove_dir_all(root).unwrap();
}
