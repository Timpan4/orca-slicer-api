use orca_slicer_api::calibration::{Calibration, CalibrationStep, rewrite_retraction};

#[tokio::test]
async fn unusable_volumetric_beds_fail_as_bad_requests_before_export() {
    use orca_slicer_api::error::AppError;
    let dir = std::env::temp_dir().join(format!("volumetric-bed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let printer = dir.join("printer.json");
    let process = dir.join("process.json");
    let filament = dir.join("filament.json");
    std::fs::write(&process, serde_json::json!({"layer_height":"0.2"}).to_string()).unwrap();
    std::fs::write(&filament, "{}").unwrap();
    let request:Calibration=serde_json::from_value(serde_json::json!({"step":"volumetric_flow","lowest":5,"highest":9,"increment":2,"baseline":7,"previous_results":{"temperature":210}})).unwrap();
    for area in [
        serde_json::json!([]),
        serde_json::json!(["bad"]),
        serde_json::json!(["0x0", "5x250"]),
        serde_json::json!(["0x0", "10x250"]),
    ] {
        std::fs::write(&printer,serde_json::json!({"printable_area":area,"nozzle_diameter":["0.4"],"gcode_flavor":"klipper","printable_height":"250"}).to_string()).unwrap();
        assert!(
            matches!(
                request.prepare(&dir, &printer, &process, &filament).await,
                Err(AppError::Bad(_))
            ),
            "{area}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
#[ignore = "requires Orca calibration resources"]
async fn flow_patterns_keep_native_objects_and_modifiers() {
    for (pattern, count) in [("yolo", 11), ("yolo_perfectionist", 16), ("pass1", 9), ("pass2", 10)]
    {
        let dir = std::env::temp_dir().join(format!("flow-{pattern}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let printer = dir.join("printer.json");
        let process = dir.join("process.json");
        let filament = dir.join("filament.json");
        std::fs::write(&printer,serde_json::json!({"printable_area":["0x0","250x0","250x250","0x250"],"nozzle_diameter":["0.4"],"gcode_flavor":"klipper","printable_height":"250"}).to_string()).unwrap();
        std::fs::write(&process,serde_json::json!({"initial_layer_print_height":"0.3","internal_solid_infill_speed":"100","top_surface_speed":"100"}).to_string()).unwrap();
        std::fs::write(
            &filament,
            serde_json::json!({"filament_max_volumetric_speed":["20"]}).to_string(),
        )
        .unwrap();
        let request:Calibration=serde_json::from_value(serde_json::json!({"step":"flow_rate","flow_pattern":pattern,"baseline":0.98,"previous_results":{"temperature":210}})).unwrap();
        let path = request.prepare(&dir, &printer, &process, &filament).await.unwrap();
        let assembly: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let objects = assembly["plates"][0]["objects"].as_array().unwrap();
        assert_eq!(objects.len(), count);
        for object in objects {
            let path = std::path::Path::new(object["path"].as_str().unwrap());
            let name = path.file_stem().unwrap().to_str().unwrap();
            let modifier: f64 =
                name.strip_prefix("flowrate_").unwrap().replace('m', "-").parse().unwrap();
            let expected = if pattern.starts_with("yolo") {
                (0.98 + modifier) / 0.98
            } else {
                1.0 + modifier / 100.0
            };
            let ratio: f64 =
                object["print_params"]["print_flow_ratio"].as_str().unwrap().parse().unwrap();
            assert_eq!(ratio, expected);
            assert_eq!(object["print_params"]["wall_loops"], "1");
            assert_eq!(object["print_params"]["top_shell_layers"], "5");
            let bytes = std::fs::read(path).unwrap();
            let top = bytes[84..]
                .chunks_exact(50)
                .flat_map(|f| {
                    [20, 32, 44].map(|s| f32::from_le_bytes(f[s..s + 4].try_into().unwrap()))
                })
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((top - 2.1).abs() <= f32::EPSILON * 2.1);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires checksum-pinned Orca CLI and resources"]
async fn native_resources_preserve_names_settings_and_cut_heights() {
    let cases = [
        ("temperature", 200.0, 210.0, 5.0, 30.0, "temperature_tower"),
        ("pressure_advance", 0.01, 0.05, 0.02, 3.0, "tower_with_seam"),
        ("retraction", 0.2, 0.6, 0.2, 3.4, "retraction_tower"),
        ("volumetric_flow", 5.0, 9.0, 2.0, 2.5, "SpeedTestStructure"),
    ];
    for (step, low, high, inc, height, name) in cases {
        let dir = std::env::temp_dir().join(format!("native-{step}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let printer = dir.join("printer.json");
        let process = dir.join("process.json");
        let filament = dir.join("filament.json");
        std::fs::write(&printer,serde_json::json!({"max_layer_height":["0.3"],"nozzle_diameter":["0.4"],"gcode_flavor":"klipper","printable_height":"250","machine_max_speed_x":["500"],"machine_max_speed_y":["500"],"printable_area":["0x0","250x0","250x250","0x250"]}).to_string()).unwrap();
        std::fs::write(&process,serde_json::json!({"layer_height":"0.2","line_width":"0.45","outer_wall_speed":"100","initial_layer_print_height":"0.2"}).to_string()).unwrap();
        std::fs::write(
            &filament,
            serde_json::json!({"filament_max_volumetric_speed":["20"],"filament_flow_ratio":["1"]})
                .to_string(),
        )
        .unwrap();
        let previous = if step == "temperature" {
            serde_json::json!({})
        } else {
            serde_json::json!({"temperature":210})
        };
        let request:Calibration=serde_json::from_value(serde_json::json!({"step":step,"lowest":low,"highest":high,"increment":inc,"baseline":if step=="temperature" {210.0} else {high},"previous_results":previous})).unwrap();
        let assembly = request.prepare(&dir, &printer, &process, &filament).await.unwrap();
        let assembly: serde_json::Value =
            serde_json::from_slice(&std::fs::read(assembly).unwrap()).unwrap();
        let path = assembly["plates"][0]["objects"][0]["path"].as_str().unwrap();
        assert!(path.ends_with(&format!("{name}.stl")));
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() > 84 + 12 * 50);
        let mut top = f32::NEG_INFINITY;
        let mut bottom = f32::INFINITY;
        for face in bytes[84..].chunks_exact(50) {
            for start in [20, 32, 44] {
                let z = f32::from_le_bytes(face[start..start + 4].try_into().unwrap());
                top = top.max(z);
                bottom = bottom.min(z);
            }
        }
        assert_eq!(bottom, 0.0);
        assert!((f64::from(top) - height).abs() <= f64::from(f32::EPSILON) * height);
        for plane in [bottom, top] {
            assert!(
                bytes[84..]
                    .chunks_exact(50)
                    .any(|face| [20, 32, 44].iter().all(|start| f32::from_le_bytes(
                        face[*start..*start + 4].try_into().unwrap()
                    ) == plane)),
                "{step} missing closed cut face at {plane}"
            );
        }
        let process: serde_json::Value =
            serde_json::from_slice(&std::fs::read(process).unwrap()).unwrap();
        if step == "retraction" {
            assert_eq!(process["bottom_shell_layers"], "3");
        }
        if step == "pressure_advance" {
            assert_eq!(process["wall_loops"], "2");
            assert_eq!(process["bottom_shell_layers"], "0");
        }
        if step == "volumetric_flow" {
            assert_eq!(process["spiral_mode"], "1");
            assert_eq!(process["bottom_shell_layers"], "0");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn native_model_contract_rejects_wrong_fields_and_temperature_labels() {
    let parse = |value| {
        serde_json::from_value::<Calibration>(value)
            .and_then(|r| r.validate().map(|_| r).map_err(serde::de::Error::custom))
    };
    for pattern in ["yolo", "yolo_perfectionist", "pass1", "pass2"] {
        assert!(
            parse(serde_json::json!({"step":"flow_rate","flow_pattern":pattern,"baseline":1}))
                .is_ok()
        );
    }
    for value in [
        serde_json::json!({"step":"flow_rate","baseline":1}),
        serde_json::json!({"step":"flow_rate","flow_pattern":"pass1","baseline":1,"lowest":0.9}),
        serde_json::json!({"step":"temperature","flow_pattern":"pass1","lowest":200,"highest":210,"increment":5,"baseline":210}),
        serde_json::json!({"step":"temperature","lowest":0,"highest":10,"increment":5,"baseline":210}),
        serde_json::json!({"step":"temperature","lowest":345,"highest":355,"increment":5,"baseline":210}),
        serde_json::json!({"step":"temperature","lowest":200,"highest":210,"increment":10,"baseline":210}),
        serde_json::json!({"step":"pressure_advance","baseline":0.02}),
    ] {
        assert!(parse(value.clone()).is_err(), "{value}");
    }
    assert!(parse(serde_json::json!({"step":"temperature","lowest":5,"highest":350,"increment":5,"baseline":210})).is_ok());
}

fn request() -> Calibration {
    Calibration {
        step: CalibrationStep::Retraction,
        lowest: 0.2,
        highest: 0.6,
        increment: 0.2,
        baseline: 0.4,
        flow_pattern: None,
        previous_results: Default::default(),
    }
}

#[test]
fn includes_decimal_endpoint_and_rejects_non_grid_upper_bound() {
    assert_eq!(request().values().unwrap().len(), 3);
    let mut request = request();
    request.highest = 0.65;
    assert!(request.values().is_err());
}

#[test]
fn paired_retracts_change_at_height_without_changing_start_end_or_xy_extrusion() {
    let input = "M83\nG1 E-0.6 F1800\nG1 E0.6 F1800\n; Z_HEIGHT: 0.6\nG1 E-.6 F1800\nG1 X30 Y0\nG1 E.6 F1800\nG1 X35 E.2\n; Z_HEIGHT: 1.6\nG1 E-.6\nG1 X0 Y0\nG1 E.6\n; Z_HEIGHT: 2.6\nG1 E-.6\nG1 X30 Y0\nG1 E.6\n;LAYERCOVE_CALIBRATION_END\nG1 E-.8\n";
    let output = rewrite_retraction(input, &request()).unwrap();
    assert!(output.contains("G1 E-0.6 F1800\nG1 E0.6 F1800"));
    for value in ["0.2", "0.4", "0.6"] {
        assert!(output.contains(&format!("G1 E-{value}")));
        assert!(output.contains(&format!("G1 E{value}")));
    }
    assert!(output.contains("G1 X35 E.2"));
    assert!(output.ends_with(";LAYERCOVE_CALIBRATION_END\nG1 E-.8\n"));
}

#[test]
fn rejects_absolute_extrusion_and_unpaired_retraction() {
    for code in [
        "M82\n; Z_HEIGHT: 0.6\nG1 E-.6\nG1 E.6\n;LAYERCOVE_CALIBRATION_END\n",
        "M83\n; Z_HEIGHT: 0.6\nG1 E-.6\n;LAYERCOVE_CALIBRATION_END\n",
    ] {
        assert!(rewrite_retraction(code, &request()).is_err());
    }
}

#[test]
fn final_shutdown_retract_does_not_count_as_a_tested_band() {
    let input = "M83\n; Z_HEIGHT: 0.6\nG1 E-.6\nG1 E.6\n; Z_HEIGHT: 1.6\nG1 E-.6\nG1 E.6\n; Z_HEIGHT: 2.6\nG1 E-.6\n;LAYERCOVE_CALIBRATION_END\n";
    assert!(rewrite_retraction(input, &request()).is_err());
}

#[test]
fn klipper_layer_markers_identify_retraction_bands() {
    let input = "M83\nG1 E-.6\n;LAYERCOVE_CALIBRATION Retraction 0.2 Z=0.6\nG1 E.6\nG1 E-.6\nG1 E.6\n;LAYERCOVE_CALIBRATION Retraction 0.4 Z=1.6\nG1 E-.6\nG1 E.6\n;LAYERCOVE_CALIBRATION Retraction 0.6 Z=2.6\nG1 E-.6\nG1 E.6\n;LAYERCOVE_CALIBRATION_END\n";
    let output = rewrite_retraction(input, &request()).unwrap();
    assert!(output.contains("G1 E-0.2\nG1 E0.2"));
    assert!(output.contains("G1 E-0.4\nG1 E0.4"));
}

// Failure mode: a profile printing at 265 °C under a parent's 220-260 °C recommended range
// could not be calibrated, though OrcaSlicer's own temperature tower allows it.
#[tokio::test]
#[ignore = "requires checksum-pinned Orca CLI"]
async fn temperature_tower_may_exceed_the_filament_recommended_range() {
    let dir = std::env::temp_dir().join(format!("calibration-range-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, value: serde_json::Value| {
        let path = dir.join(name);
        std::fs::write(&path, value.to_string()).unwrap();
        path
    };
    let printer = write(
        "printer.json",
        serde_json::json!({"nozzle_diameter":["0.4"],"gcode_flavor":"klipper","printable_height":"250"}),
    );
    let process = write("process.json", serde_json::json!({"layer_height":"0.2"}));
    let filament =
        write("filament.json", serde_json::json!({"nozzle_temperature_range_high":["260"]}));
    let tower = |lowest: f64, highest: f64, baseline: f64| Calibration {
        step: CalibrationStep::Temperature,
        lowest,
        highest,
        increment: 5.0,
        baseline,
        flow_pattern: None,
        previous_results: Default::default(),
    };
    tower(240.0, 270.0, 265.0).prepare(&dir, &printer, &process, &filament).await.unwrap();
    assert!(tower(340.0, 355.0, 350.0).validate().is_err());
    let error =
        tower(490.0, 505.0, 495.0).prepare(&dir, &printer, &process, &filament).await.unwrap_err();
    assert!(error.to_string().contains("350"), "{error}");
    std::fs::remove_dir_all(dir).unwrap();
}

// Failure mode: a first layer line width of "160%" made the pinned Orca CLI's arrange read a
// 160 mm skirt (get_real_skirt_dist uses the raw number), shrink the bed to nothing, and exit -21.
#[tokio::test]
#[ignore = "requires checksum-pinned Orca CLI"]
async fn percentage_line_widths_become_millimetres_of_the_nozzle() {
    let dir = std::env::temp_dir().join(format!("calibration-widths-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, value: serde_json::Value| {
        let path = dir.join(name);
        std::fs::write(&path, value.to_string()).unwrap();
        path
    };
    let printer = write(
        "printer.json",
        serde_json::json!({"nozzle_diameter":["0.4"],"gcode_flavor":"klipper","printable_height":"250"}),
    );
    let process = write(
        "process.json",
        serde_json::json!({"layer_height":"0.2","initial_layer_line_width":" 160 %","line_width":["110%"],"inner_wall_line_width":"0.45"}),
    );
    let filament = write("filament.json", serde_json::json!({}));
    let tower = Calibration {
        step: CalibrationStep::Temperature,
        lowest: 240.0,
        highest: 270.0,
        increment: 5.0,
        baseline: 265.0,
        flow_pattern: None,
        previous_results: Default::default(),
    };
    tower.prepare(&dir, &printer, &process, &filament).await.unwrap();
    let prepared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&process).unwrap()).unwrap();
    assert_eq!(prepared["initial_layer_line_width"], "0.64");
    assert_eq!(prepared["line_width"], serde_json::json!(["0.44"]));
    assert_eq!(prepared["inner_wall_line_width"], "0.45");
    // A profile that leaves a width to its parent keeps leaving it.
    let process = write("process.json", serde_json::json!({"layer_height":"0.2"}));
    tower.prepare(&dir, &printer, &process, &filament).await.unwrap();
    let prepared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&process).unwrap()).unwrap();
    assert!(prepared.get("line_width").is_none(), "{prepared}");
    std::fs::remove_dir_all(dir).unwrap();
}
