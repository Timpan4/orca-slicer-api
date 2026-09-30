use orca_slicer_api::{
    contract::compact_json,
    schema::{OrcaSources, build_process_schema, build_profile_schema},
};
use std::{collections::HashSet, fs, path::PathBuf};

#[test]
#[ignore = "requires ORCA_SOURCE_DIR at pinned commit"]
fn pinned_orca_process_schema_is_complete_and_deterministic() {
    let root =
        PathBuf::from(std::env::var_os("ORCA_SOURCE_DIR").expect("ORCA_SOURCE_DIR required"));
    let print_config = fs::read_to_string(root.join("src/libslic3r/PrintConfig.cpp")).unwrap();
    let print_config_header =
        fs::read_to_string(root.join("src/libslic3r/PrintConfig.hpp")).unwrap();
    let constants =
        fs::read_to_string(root.join("src/libslic3r/PrintConfigConstants.hpp")).unwrap();
    let tab_source = fs::read_to_string(root.join("src/slic3r/GUI/Tab.cpp")).unwrap();
    let build = || {
        build_process_schema(
            OrcaSources {
                print_config: &print_config,
                print_config_header: &print_config_header,
                constants: &constants,
            },
            &tab_source,
        )
        .unwrap()
    };
    let first = build();
    let second = build();
    assert_eq!(
        compact_json(&serde_json::to_value(&first).unwrap()).unwrap(),
        compact_json(&serde_json::to_value(&second).unwrap()).unwrap()
    );

    assert_eq!(first.pages.len(), 6);
    assert_eq!(first.pages.iter().map(|page| page.groups.len()).sum::<usize>(), 41);
    let placed = first
        .pages
        .iter()
        .flat_map(|page| &page.groups)
        .flat_map(|group| &group.options)
        .collect::<Vec<_>>();
    assert_eq!(placed.len(), 342);
    assert_eq!(placed.iter().copied().collect::<HashSet<_>>().len(), 342);
    assert_eq!(first.options.len(), 342);
    assert_eq!(first.scopes.len(), 342);
    assert_eq!(first.samples.len(), 342);
    let metadata =
        first.options.iter().map(|option| option["key"].as_str().unwrap()).collect::<HashSet<_>>();
    assert_eq!(metadata, placed.iter().map(|key| key.as_str()).collect());
    assert!(first.options.iter().all(|option| {
        ["key", "type", "label", "tooltip", "mode", "units", "nullable", "default"]
            .iter()
            .all(|key| option.get(key).is_some())
    }));
    let bottom_layers = first
        .options
        .iter()
        .find(|option| option["key"] == "support_interface_bottom_layers")
        .unwrap();
    assert_eq!(bottom_layers["choices"], serde_json::json!([-1, 0, 1, 2, 3]));
}

#[test]
#[ignore = "requires ORCA_SOURCE_DIR at pinned commit"]
fn pinned_profile_schemas_include_motion_and_filament_override_metadata() {
    let root = PathBuf::from(std::env::var_os("ORCA_SOURCE_DIR").unwrap());
    let config = fs::read_to_string(root.join("src/libslic3r/PrintConfig.cpp")).unwrap();
    let header = fs::read_to_string(root.join("src/libslic3r/PrintConfig.hpp")).unwrap();
    let constants =
        fs::read_to_string(root.join("src/libslic3r/PrintConfigConstants.hpp")).unwrap();
    let presets = fs::read_to_string(root.join("src/libslic3r/Preset.cpp")).unwrap();
    for (kind, count) in [("printer", 160), ("filament", 126)] {
        let build = || {
            build_profile_schema(
                OrcaSources {
                    print_config: &config,
                    print_config_header: &header,
                    constants: &constants,
                },
                &presets,
                kind,
            )
            .unwrap()
        };
        let schema = build();
        assert_eq!(schema.options.len(), count);
        assert_eq!(schema.scopes.len(), count);
        assert_eq!(schema.samples.len(), count);
        assert_eq!(
            compact_json(&serde_json::to_value(&schema).unwrap()).unwrap(),
            compact_json(&serde_json::to_value(build()).unwrap()).unwrap()
        );
        let find = |key: &str| schema.options.iter().find(|option| option["key"] == key).unwrap();
        if kind == "printer" {
            assert_eq!(find("machine_max_speed_x")["default"], serde_json::json!([500.0, 200.0]));
            assert_eq!(find("machine_max_speed_e")["units"], "mm/s");
            assert_eq!(find("printable_area")["item_type"], "point");
        } else {
            assert_eq!(find("nozzle_temperature")["item_type"], "int");
            assert_eq!(find("nozzle_temperature")["max"].as_f64(), Some(1500.0));
            assert_eq!(find("pressure_advance")["max"].as_f64(), Some(2.0));
            assert_eq!(find("filament_retraction_length")["nullable"], true);
            assert_eq!(find("enable_pressure_advance")["default"], serde_json::json!([false]));
        }
    }
}
