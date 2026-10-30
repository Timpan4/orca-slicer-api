use orca_slicer_api::calibration::{Calibration, CalibrationStep, rewrite_retraction};

fn request() -> Calibration {
    Calibration {
        step: CalibrationStep::Retraction,
        lowest: 0.2,
        highest: 0.6,
        increment: 0.2,
        baseline: 0.4,
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
    let process = write("process.json", serde_json::json!({}));
    let filament =
        write("filament.json", serde_json::json!({"nozzle_temperature_range_high":["260"]}));
    let tower = |lowest: f64, highest: f64, baseline: f64| Calibration {
        step: CalibrationStep::Temperature,
        lowest,
        highest,
        increment: 5.0,
        baseline,
        previous_results: Default::default(),
    };
    tower(240.0, 270.0, 265.0).prepare(&dir, &printer, &process, &filament).await.unwrap();
    tower(480.0, 500.0, 490.0).prepare(&dir, &printer, &process, &filament).await.unwrap();
    let error =
        tower(490.0, 505.0, 495.0).prepare(&dir, &printer, &process, &filament).await.unwrap_err();
    assert!(error.to_string().contains("500"), "{error}");
    std::fs::remove_dir_all(dir).unwrap();
}

// Failure mode: a first layer line width of "160%" made the pinned Orca CLI's arrange read a
// 160 mm skirt (get_real_skirt_dist uses the raw number), shrink the bed to nothing, and exit -21.
#[tokio::test]
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
        serde_json::json!({"initial_layer_line_width":" 160 %","line_width":["110%"],"inner_wall_line_width":"0.45"}),
    );
    let filament = write("filament.json", serde_json::json!({}));
    let tower = Calibration {
        step: CalibrationStep::Temperature,
        lowest: 240.0,
        highest: 270.0,
        increment: 5.0,
        baseline: 265.0,
        previous_results: Default::default(),
    };
    tower.prepare(&dir, &printer, &process, &filament).await.unwrap();
    let prepared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&process).unwrap()).unwrap();
    assert_eq!(prepared["initial_layer_line_width"], "0.64");
    assert_eq!(prepared["line_width"], serde_json::json!(["0.44"]));
    assert_eq!(prepared["inner_wall_line_width"], "0.45");
    // A profile that leaves a width to its parent keeps leaving it.
    let process = write("process.json", serde_json::json!({}));
    tower.prepare(&dir, &printer, &process, &filament).await.unwrap();
    let prepared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&process).unwrap()).unwrap();
    assert!(prepared.get("line_width").is_none(), "{prepared}");
    std::fs::remove_dir_all(dir).unwrap();
}
