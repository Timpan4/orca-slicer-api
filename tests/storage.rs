use orca_slicer_api::storage::{job_dir, reset_jobs};
use std::fs;

#[tokio::test]
async fn startup_recovery_removes_only_abandoned_jobs() {
    let root = std::env::temp_dir().join(format!("orca-storage-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("cache")).unwrap();
    fs::write(root.join("cache/keep"), b"cache").unwrap();
    let abandoned = job_dir(&root, "abandoned").await.unwrap();
    fs::write(abandoned.join("model.stl"), b"stale").unwrap();

    reset_jobs(&root).await.unwrap();

    assert!(!abandoned.exists());
    assert_eq!(fs::read(root.join("cache/keep")).unwrap(), b"cache");
    assert!(root.join("jobs").is_dir());
    fs::remove_dir_all(root).unwrap();
}
