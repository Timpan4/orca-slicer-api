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
    let error =
        tower(340.0, 355.0, 345.0).prepare(&dir, &printer, &process, &filament).await.unwrap_err();
    assert!(error.to_string().contains("350"), "{error}");
    std::fs::remove_dir_all(dir).unwrap();
}
