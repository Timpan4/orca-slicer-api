use orca_slicer_api::profiles::{load_profile_catalog, load_profile_index};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

#[test]
fn profile_index_includes_resolved_content_metadata() {
    let root = std::env::temp_dir().join(format!("orca-profile-metadata-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("vendor/process")).unwrap();
    fs::write(
        root.join("vendor/process/base.json"),
        serde_json::to_vec(&json!({
            "type": "process",
            "name": "Base process",
            "setting_id": "base",
            "instantiation": "false",
            "compatible_printers": ["Dremel 3D40"]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor/process/child.json"),
        serde_json::to_vec(&json!({
            "type": "process",
            "name": "Dremel process",
            "setting_id": "child",
            "inherits": "Base process",
            "layer_height": 0.2
        }))
        .unwrap(),
    )
    .unwrap();
    let catalog = load_profile_catalog(&root).unwrap();
    let index = json!({
        "printer": [],
        "process": [{"name": "Dremel process", "base_id": "child"}],
        "filament": []
    });
    let enriched = catalog.decorate_index(index).unwrap();
    let entry = &enriched["process"][0];
    assert_eq!(entry["stable_id"], "process:vendor:child");
    assert_eq!(entry["content"]["compatible_printers"], json!(["Dremel 3D40"]));
    assert_eq!(entry["content"]["layer_height"], 0.2);
    let canonical = br#"{"compatible_printers":["Dremel 3D40"],"inherits":"Base process","layer_height":0.2,"name":"Dremel process","setting_id":"child","type":"process"}"#;
    let expected_hash =
        Sha256::digest(canonical).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    assert_eq!(entry["content_hash"], expected_hash);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inherited_profiles_use_nearest_directory_parent() {
    let root =
        std::env::temp_dir().join(format!("orca-profile-nested-parent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("vendor/machine/HSN")).unwrap();
    for (path, marker) in
        [("vendor/machine/base.json", "root"), ("vendor/machine/HSN/base.json", "nested")]
    {
        fs::write(
            root.join(path),
            serde_json::to_vec(&json!({
                "type": "machine",
                "name": "fdm_klipper_common",
                "instantiation": "false",
                "marker": marker
            }))
            .unwrap(),
        )
        .unwrap();
    }
    for (path, name, setting_id) in [
        ("vendor/machine/root.json", "Root printer", "root-printer"),
        ("vendor/machine/HSN/child.json", "HSN printer", "hsn-printer"),
    ] {
        fs::write(
            root.join(path),
            serde_json::to_vec(&json!({
                "type": "machine",
                "name": name,
                "setting_id": setting_id,
                "inherits": "fdm_klipper_common"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    let catalog = load_profile_catalog(&root).unwrap();
    let enriched = catalog
        .decorate_index(json!({
            "printer": [
                {"name": "Root printer", "base_id": "root-printer"},
                {"name": "HSN printer", "base_id": "hsn-printer"}
            ],
            "process": [],
            "filament": []
        }))
        .unwrap();
    assert_eq!(enriched["printer"][0]["content"]["marker"], "root");
    assert_eq!(enriched["printer"][1]["content"]["marker"], "nested");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inherited_profiles_continue_in_resolved_parent_namespace() {
    let root =
        std::env::temp_dir().join(format!("orca-profile-parent-namespace-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for directory in [
        "consumer/filament",
        "OrcaFilamentLibrary/filament/Brand",
        "OrcaFilamentLibrary/filament/base",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for (path, marker) in [
        ("consumer/filament/common.json", "consumer"),
        ("OrcaFilamentLibrary/filament/base/common.json", "library"),
    ] {
        fs::write(
            root.join(path),
            serde_json::to_vec(&json!({
                "type": "filament",
                "name": "Common base",
                "instantiation": "false",
                "marker": marker
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fs::write(
        root.join("OrcaFilamentLibrary/filament/Brand/base.json"),
        serde_json::to_vec(&json!({
            "type": "filament",
            "name": "Brand base",
            "inherits": "Common base",
            "instantiation": "false"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("consumer/filament/child.json"),
        serde_json::to_vec(&json!({
            "type": "filament",
            "name": "Consumer preset",
            "setting_id": "consumer-preset",
            "inherits": "Brand base"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("consumer.json"),
        serde_json::to_vec(&json!({
            "name": "consumer",
            "filament_list": [
                {"name": "Consumer preset", "sub_path": "filament/child.json"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("OrcaFilamentLibrary.json"),
        serde_json::to_vec(&json!({
            "name": "OrcaFilamentLibrary",
            "filament_list": [
                {"name": "Common base", "sub_path": "filament/base/common.json"},
                {"name": "Brand base", "sub_path": "filament/Brand/base.json"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let catalog = load_profile_catalog(&root).unwrap();
    let enriched = catalog
        .decorate_index(json!({
            "printer": [],
            "process": [],
            "filament": [{"name": "Consumer preset", "base_id": "consumer-preset"}]
        }))
        .unwrap();
    assert_eq!(enriched["filament"][0]["content"]["marker"], "library");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inherited_profiles_prefer_exact_filename_over_legacy_duplicate() {
    let root =
        std::env::temp_dir().join(format!("orca-profile-filename-parent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("vendor/machine")).unwrap();
    for (file, marker) in
        [("fdm_machine_common.json", "manifest"), ("_fdm_machine_common.json", "legacy")]
    {
        fs::write(
            root.join("vendor/machine").join(file),
            serde_json::to_vec(&json!({
                "type": "machine",
                "name": "fdm_machine_common",
                "instantiation": "false",
                "marker": marker
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fs::write(
        root.join("vendor/machine/child.json"),
        serde_json::to_vec(&json!({
            "type": "machine",
            "name": "Printer",
            "setting_id": "printer",
            "inherits": "fdm_machine_common"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("vendor.json"),
        serde_json::to_vec(&json!({
            "name": "vendor",
            "machine_list": [
                {"name": "fdm_machine_common", "sub_path": "machine/fdm_machine_common.json"},
                {"name": "Printer", "sub_path": "machine/child.json"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let catalog = load_profile_catalog(&root).unwrap();
    let enriched = catalog
        .decorate_index(json!({
            "printer": [{"name": "Printer", "base_id": "printer"}],
            "process": [],
            "filament": []
        }))
        .unwrap();
    assert_eq!(enriched["printer"][0]["content"]["marker"], "manifest");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn profile_index_rejects_ambiguous_active_inheritance() {
    let root =
        std::env::temp_dir().join(format!("orca-profile-index-ambiguous-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for directory in ["a/process", "b/process", "c/process"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for path in ["a/process/parent-a.json", "b/process/parent-b.json"] {
        fs::write(
            root.join(path),
            serde_json::to_vec(&json!({
                "type": "process",
                "name": "Shared parent",
                "instantiation": "false"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fs::write(
        root.join("c/process/child.json"),
        serde_json::to_vec(&json!({
            "type": "process",
            "name": "Active child",
            "setting_id": "child",
            "inherits": "Shared parent"
        }))
        .unwrap(),
    )
    .unwrap();
    let output_path = root.join("index.json");

    let output = Command::new(env!("CARGO_BIN_EXE_profile-index"))
        .args(["--profiles", root.to_str().unwrap(), "--output", output_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("failed to resolve bundled process profile Active child"));
    assert!(stderr.contains("ambiguous bundled process parent profile: Shared parent"));
    fs::remove_dir_all(root).unwrap();
}

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
