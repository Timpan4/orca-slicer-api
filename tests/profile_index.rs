use orca_slicer_api::profiles::load_profile_index;
use serde_json::json;
use std::{fs, process::Command};

#[test]
fn profile_index_accepts_generated_filament_count() {
    let path = std::env::temp_dir().join(format!(
        "orca-profile-index-generated-{}-{}.json",
        std::process::id(),
        7052
    ));
    let index = json!({
        "printer": [],
        "process": [],
        "filament": (0..7052)
            .map(|i| json!({"name": format!("Filament {i}"), "base_id": format!("f{i}")}))
            .collect::<Vec<_>>(),
    });
    fs::write(&path, serde_json::to_vec(&index).unwrap()).unwrap();

    assert_eq!(load_profile_index(&path).unwrap(), index);
    fs::remove_file(path).unwrap();
}

#[test]
fn profile_index_is_sorted_and_valid() {
    let root = std::env::temp_dir().join(format!("orca-profile-index-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("vendor/process")).unwrap();
    fs::create_dir_all(root.join("vendor/machine")).unwrap();
    fs::write(
        root.join("vendor/process/z.json"),
        serde_json::to_vec(&json!({"type":"process","name":"Z profile","setting_id":"z"})).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor/process/a.json"),
        serde_json::to_vec(&json!({"type":"process","name":"A profile","setting_id":"a"})).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor/process/abstract.json"),
        serde_json::to_vec(&json!({
            "type":"process",
            "name":"Abstract profile",
            "setting_id":"abstract",
            "instantiation":"false"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor/process/usable.json"),
        serde_json::to_vec(&json!({
            "type":"process",
            "name":"Usable profile",
            "setting_id":"usable",
            "instantiation":"true"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor/machine/printer.json"),
        serde_json::to_vec(&json!({"type":"machine","name":"Printer","setting_id":"p"})).unwrap(),
    )
    .unwrap();
    let first = root.join("first.json");
    let second = root.join("second.json");

    for output in [&first, &second] {
        let status = Command::new(env!("CARGO_BIN_EXE_profile-index"))
            .args(["--profiles", root.to_str().unwrap(), "--output", output.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let index = load_profile_index(&first).unwrap();
    assert_eq!(index["printer"][0]["base_id"], "p");
    assert_eq!(index["process"][0]["base_id"], "a");
    assert_eq!(index["process"][1]["base_id"], "usable");
    assert_eq!(index["process"][2]["base_id"], "z");
    assert!(
        index["process"].as_array().unwrap().iter().all(|entry| { entry["base_id"] != "abstract" })
    );
    assert!(index["filament"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
