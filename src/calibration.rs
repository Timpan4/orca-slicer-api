use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationStep {
    Temperature,
    FlowRate,
    PressureAdvance,
    Retraction,
    VolumetricFlow,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub step: CalibrationStep,
    #[serde(default = "missing", skip_serializing_if = "is_missing")]
    pub lowest: f64,
    #[serde(default = "missing", skip_serializing_if = "is_missing")]
    pub highest: f64,
    #[serde(default = "missing", skip_serializing_if = "is_missing")]
    pub increment: f64,
    #[serde(default, deserialize_with = "flow_pattern", skip_serializing_if = "Option::is_none")]
    pub flow_pattern: Option<FlowPattern>,
    pub baseline: f64,
    #[serde(default)]
    pub previous_results: std::collections::BTreeMap<CalibrationStep, f64>,
}

fn missing() -> f64 {
    f64::NAN
}
fn flow_pattern<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FlowPattern>, D::Error> {
    FlowPattern::deserialize(deserializer).map(Some)
}
fn is_missing(v: &f64) -> bool {
    v.is_nan()
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowPattern {
    Yolo,
    YoloPerfectionist,
    Pass1,
    Pass2,
}
impl Calibration {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.previous_results.iter().any(|(step, value)| {
            *step >= self.step
                || !value.is_finite()
                || *value < 0.0
                || (*value == 0.0
                    && !matches!(
                        step,
                        CalibrationStep::PressureAdvance | CalibrationStep::Retraction
                    ))
                || (matches!(step, CalibrationStep::Temperature) && value.fract() != 0.0)
        }) {
            return Err(AppError::Bad("invalid preceding calibration results".into()));
        }
        if self.step == CalibrationStep::FlowRate {
            if self.flow_pattern.is_none()
                || ![self.lowest, self.highest, self.increment].iter().all(|v| v.is_nan())
                || !self.baseline.is_finite()
                || self.baseline <= 0.0
                || self.baseline > 2.0
            {
                return Err(AppError::Bad("flow_rate requires flow_pattern and positive baseline at most 2; range fields are forbidden".into()));
            }
            return Ok(());
        }
        if self.flow_pattern.is_some() {
            return Err(AppError::Bad("flow_pattern is only valid for flow_rate".into()));
        }
        if self.step == CalibrationStep::Temperature
            && (self.increment != 5.0
                || self.lowest < 5.0
                || self.highest > 350.0
                || self.lowest % 5.0 != 0.0
                || self.highest % 5.0 != 0.0)
        {
            return Err(AppError::Bad(
                "temperature requires whole degrees, increment 5 and bounds in 5..350, multiples of 5".into(),
            ));
        }
        if ![self.lowest, self.highest, self.increment, self.baseline]
            .iter()
            .all(|value| value.is_finite())
            || self.lowest < 0.0
            || self.highest <= self.lowest
            || self.increment <= 0.0
            || self.increment > self.highest - self.lowest
            || self.baseline < 0.0
        {
            return Err(AppError::Bad("invalid calibration test values".into()));
        }
        if matches!(self.step, CalibrationStep::Temperature)
            && [self.lowest, self.highest, self.increment, self.baseline]
                .iter()
                .any(|value| value.fract() != 0.0)
        {
            return Err(AppError::Bad("temperature test values must be whole degrees".into()));
        }
        if matches!(
            self.step,
            CalibrationStep::Temperature
                | CalibrationStep::FlowRate
                | CalibrationStep::VolumetricFlow
        ) && (self.lowest == 0.0 || self.baseline == 0.0)
        {
            return Err(AppError::Bad(
                "this calibration requires positive test and baseline values".into(),
            ));
        }
        Ok(())
    }

    pub fn values(&self) -> Result<Vec<f64>, AppError> {
        self.validate()?;
        if self.step == CalibrationStep::FlowRate {
            return Ok(Vec::new());
        }
        let intervals = ((self.highest - self.lowest) / self.increment).ceil();
        // Generated objects/ranges use the existing model-state and JSON entry contracts.
        let maximum = if self.step == CalibrationStep::FlowRate { 256.0 } else { 4096.0 };
        if !intervals.is_finite() || intervals + 1.0 > maximum {
            return Err(AppError::Bad(
                "calibration exceeds the model object/range contract".into(),
            ));
        }
        // The finite nonnegative count is bounded above before conversion/allocation.
        let values: Vec<_> = (0..=intervals as u32)
            .map(|i| self.lowest + f64::from(i) * self.increment)
            .take_while(|value| *value <= self.highest.next_up())
            .collect();
        if values.last().is_none_or(|value| *value < self.highest.next_down()) {
            return Err(AppError::Bad(
                "highest test value must be reached by the step size".into(),
            ));
        }
        Ok(values)
    }

    pub async fn prepare(
        &self,
        dir: &Path,
        printer_path: &Path,
        process_path: &Path,
        filament_path: &Path,
    ) -> Result<PathBuf, AppError> {
        self.prepare_with_cli(
            dir,
            printer_path,
            process_path,
            filament_path,
            &crate::config::cli_path(),
        )
        .await
    }

    pub(crate) async fn prepare_with_cli(
        &self,
        dir: &Path,
        printer_path: &Path,
        process_path: &Path,
        filament_path: &Path,
        cli: &str,
    ) -> Result<PathBuf, AppError> {
        self.values()?;
        let mut printer: Value = serde_json::from_slice(&tokio::fs::read(printer_path).await?)?;
        let mut process: Value = serde_json::from_slice(&tokio::fs::read(process_path).await?)?;
        let mut filament: Value = serde_json::from_slice(&tokio::fs::read(filament_path).await?)?;
        let volumetric_span = if self.step == CalibrationStep::VolumetricFlow {
            Some(volumetric_bed_span(&printer)?)
        } else {
            None
        };
        let temperature = if self.step == CalibrationStep::Temperature {
            self.highest.max(self.baseline)
        } else {
            *self
                .previous_results
                .get(&CalibrationStep::Temperature)
                .ok_or_else(|| AppError::Bad("temperature result is required".into()))?
        };
        // The filament's temperature range is a recommendation, as in OrcaSlicer, whose
        // temperature dialog (calib_dlg.cpp at the pinned source commit) only limits the start to 500 °C.
        if temperature > 500.0 {
            return Err(AppError::Bad("calibration temperature cannot exceed 500 °C".into()));
        }
        for (step, value) in self
            .previous_results
            .iter()
            .map(|(step, value)| (*step, *value))
            .chain([(self.step, self.highest.max(self.baseline))])
        {
            if matches!(step, CalibrationStep::FlowRate | CalibrationStep::PressureAdvance)
                && value > 2.0
            {
                return Err(AppError::Bad(
                    "calibration exceeds the pinned Orca setting maximum of 2".into(),
                ));
            }
        }
        let nozzle = profile_number(&printer, "nozzle_diameter")?;
        let layer_height = match self.step {
            CalibrationStep::Retraction => 0.2,
            CalibrationStep::VolumetricFlow => nozzle * 0.8,
            CalibrationStep::FlowRate => nozzle / 2.0,
            _ => profile_number(&process, "layer_height")?,
        };
        let flavor = printer["gcode_flavor"]
            .as_str()
            .ok_or_else(|| AppError::Bad("printer gcode_flavor is required".into()))?
            .to_owned();
        if !matches!(flavor.as_str(), "klipper" | "marlin" | "marlin2" | "reprapfirmware" | "bambu")
        {
            return Err(AppError::Bad(
                "calibration supports Klipper, Marlin, RepRapFirmware, and Bambu profiles".into(),
            ));
        }
        if printer["nozzle_diameter"].as_array().is_none_or(|values| values.len() != 1) {
            return Err(AppError::Bad(
                "calibration requires a single-extruder printer profile".into(),
            ));
        }
        for (step, result) in &self.previous_results {
            set_result(&mut filament, *step, *result);
        }
        set_result(&mut filament, self.step, self.baseline);
        // Arrange sizes the skirt from these widths (get_real_skirt_dist in PrintConfig.cpp at the
        // pinned commit) using the raw number, so "160%" would read as 160 mm and fill the bed.
        // Orca defines both as a ratio over the nozzle diameter, so millimetres print the same.
        for key in ["initial_layer_line_width", "line_width"] {
            // Like ConfigOptionFloatOrPercent::deserialize, a "%" anywhere marks a ratio, and the
            // number is read from the start after any whitespace, so "160 %" is also 160%.
            let resolve = |value: &mut Value| {
                if let Some(ratio) = value
                    .as_str()
                    .filter(|text| text.contains('%'))
                    .map(|text| {
                        let text = text.trim_start();
                        let end = text
                            .find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c)))
                            .unwrap_or(text.len());
                        &text[..end]
                    })
                    .and_then(|ratio| ratio.parse::<f64>().ok())
                {
                    *value = decimal(nozzle * ratio / 100.0).into();
                }
            };
            match process.get_mut(key) {
                Some(Value::Array(values)) => values.iter_mut().for_each(resolve),
                Some(value) => resolve(value),
                None => {}
            }
        }
        set(&mut process, "layer_height", layer_height);
        if self.step == CalibrationStep::Retraction {
            set(&mut process, "initial_layer_print_height", layer_height);
        }
        set(&mut process, "alternate_extra_wall", false);
        set(&mut process, "seam_slope_type", "none");
        set(&mut process, "enable_wrapping_detection", false);
        set(&mut process, "max_volumetric_extrusion_rate_slope", 0);
        set(&mut process, "enable_support", false);
        set(&mut process, "ironing_type", "no ironing");
        set(&mut process, "print_sequence", "by layer");
        set(&mut printer, "resonance_avoidance", false);
        let height = match self.step {
            CalibrationStep::Temperature => ((self.highest - self.lowest) / 5.0 + 1.0) * 10.0,
            CalibrationStep::PressureAdvance => {
                ((self.highest - self.lowest) / self.increment).ceil() + 1.0
            }
            CalibrationStep::Retraction => 1.4 + (self.highest - self.lowest) / self.increment,
            CalibrationStep::VolumetricFlow => (self.highest - self.lowest + 1.0) / self.increment,
            CalibrationStep::FlowRate => layer_height * 10.0,
        };
        if height > profile_number(&printer, "printable_height")? {
            return Err(AppError::Bad(
                "calibration tower exceeds the printer's printable height".into(),
            ));
        }
        if self.step == CalibrationStep::PressureAdvance
            && self.lowest + height.floor() * self.increment > 2.0
        {
            return Err(AppError::Bad(
                "per-height pressure advance exceeds the pinned Orca maximum of 2".into(),
            ));
        }
        let mut objects = Vec::new();
        let mut layer_code = String::new();
        if self.step == CalibrationStep::FlowRate {
            let pattern = self.flow_pattern.ok_or(AppError::Internal)?;
            let (file, linear, pass2) = match pattern {
                FlowPattern::Yolo => ("Orca-LinearFlow.3mf", true, false),
                FlowPattern::YoloPerfectionist => ("Orca-LinearFlow_fine.3mf", true, true),
                FlowPattern::Pass1 => ("flowrate-test-pass1.3mf", false, false),
                FlowPattern::Pass2 => ("flowrate-test-pass2.3mf", false, true),
            };
            let root = std::env::var("ORCA_CALIBRATION_PATH")
                .unwrap_or_else(|_| "/app/orca/resources/calib".into());
            let first = profile_number(&process, "initial_layer_print_height")?.max(layer_height);
            set(&mut process, "initial_layer_print_height", first);
            set(&mut process, "reduce_crossing_wall", true);
            let flow_area =
                layer_height * (nozzle * 1.2 - layer_height * (1.0 - std::f64::consts::PI / 4.0));
            let factor = if linear {
                (self.baseline + if pass2 { 0.035 } else { 0.05 }) / self.baseline
            } else if pass2 {
                1.0
            } else {
                1.2
            };
            let maximum =
                profile_number(&filament, "filament_max_volumetric_speed")? / (flow_area * factor);
            let solid =
                profile_number(&process, "internal_solid_infill_speed")?.min(maximum).floor();
            let top = profile_number(&process, "top_surface_speed")?.min(maximum).floor();
            let models = crate::calibration_mesh::flow_models(
                &Path::new(&root).join("filament_flow").join(file),
            )?;
            let bed = bed_bounds(&printer)?;
            let xy = if nozzle / 0.6 > 1.2 { nozzle / 0.6 } else { 1.0 };
            let mut low = [f64::INFINITY; 2];
            let mut high = [f64::NEG_INFINITY; 2];
            for (_, mesh) in &models {
                let (a, b) = mesh.bounds();
                for i in 0..2 {
                    low[i] = low[i].min(a[i] * xy);
                    high[i] = high[i].max(b[i] * xy);
                }
            }
            let shift =
                std::array::from_fn::<_, 2, _>(|i| (bed.0[i] + bed.1[i] - low[i] - high[i]) / 2.0);
            for i in 0..2 {
                if high[i] - low[i] > bed.1[i] - bed.0[i] {
                    return Err(AppError::Bad("Orca flow pattern exceeds printable area".into()));
                }
            }
            for (name, mut mesh) in models {
                let modifier = name
                    .strip_prefix("flowrate_")
                    .ok_or(AppError::Internal)?
                    .replacen('m', "-", 1)
                    .parse::<f64>()
                    .map_err(|_| AppError::Internal)?;
                let ratio = if linear {
                    (self.baseline + modifier) / self.baseline
                } else {
                    1.0 + modifier / 100.0
                };
                if ratio <= 0.0 || self.baseline * ratio > 2.0 {
                    return Err(AppError::Bad(
                        "flow sample exceeds the pinned Orca flow maximum of 2".into(),
                    ));
                }
                mesh.scale([xy, xy, (first + 9.0 * layer_height) / 2.0]);
                for p in mesh.0.iter_mut().flatten() {
                    for i in 0..2 {
                        p[i] += shift[i];
                    }
                }
                mesh.on_bed();
                if mesh.bounds().1[2] > profile_number(&printer, "printable_height")? {
                    return Err(AppError::Bad("flow model exceeds printable height".into()));
                }
                let path = dir.join(format!("{name}.stl"));
                mesh.write(&path).await?;
                let params = json!({
                    "wall_loops":"1","only_one_wall_top":"1","thick_internal_bridges":"0","enable_extra_bridge_layer":"disabled",
                    "internal_bridge_density":"100%","sparse_infill_density":"35%","min_width_top_surface":"100%",
                    "bottom_shell_layers":"2","top_shell_layers":"5","top_shell_thickness":"0","bottom_shell_thickness":"0",
                    "detect_thin_wall":"1","filter_out_gap_fill":"0","sparse_infill_pattern":"rectilinear",
                    "top_surface_line_width":(nozzle*1.2).to_string(),"internal_solid_infill_line_width":(nozzle*1.2).to_string(),
                    "top_surface_pattern":"monotonic","top_solid_infill_flow_ratio":"1","infill_direction":"45","solid_infill_direction":"135",
                    "rotate_solid_infill_direction":"1","ironing_type":"no ironing","internal_solid_infill_speed":solid.to_string(),
                    "top_surface_speed":top.to_string(),"seam_slope_type":"none","print_flow_ratio":ratio.to_string()
                });
                objects.push(json!({"path":path,"count":1,"filaments":[1],"print_params":params}));
            }
        } else {
            let resource = match self.step {
                CalibrationStep::Temperature => "temperature_tower/temperature_tower.drc",
                CalibrationStep::PressureAdvance => "pressure_advance/tower_with_seam.drc",
                CalibrationStep::Retraction => "retraction/retraction_tower.drc",
                CalibrationStep::VolumetricFlow => "volumetric_speed/SpeedTestStructure.drc",
                CalibrationStep::FlowRate => unreachable!(),
            };
            let mut mesh = crate::calibration_mesh::Mesh::tower(dir, resource, cli).await?;
            mesh.on_bed();
            if self.step == CalibrationStep::Temperature {
                let bottom = (350.0 - self.highest) / 5.0 * 10.0;
                // Plater::calib_temp uses libslic3r's EPSILON (1e-4 mm).
                mesh.cut(bottom + height + 1e-4, true)?;
                if bottom > 0.0 {
                    mesh.cut(bottom + 1e-4, false)?;
                }
                mesh.on_bed();
                set_result(&mut filament, CalibrationStep::Temperature, self.highest);
                set(&mut process, "brim_type", "outer_only");
                set(&mut process, "brim_object_gap", 0);
                set(&mut process, "brim_width", 5.0);
            } else {
                if height > mesh.bounds().1[2] {
                    return Err(AppError::Bad(
                        "requested height exceeds Orca calibration model".into(),
                    ));
                }
                mesh.cut(height, true)?;
                set(&mut process, "top_shell_layers", 0);
                set(&mut process, "sparse_infill_density", "0%");
                match self.step {
                    CalibrationStep::PressureAdvance => {
                        let configured = profile_nonnegative_number(&process, "line_width")?;
                        let width = if configured == 0.0 { nozzle * 1.125 } else { configured };
                        let area = layer_height
                            * (width - layer_height * (1.0 - std::f64::consts::PI / 4.0));
                        // calib.cpp's suggested minimum is 100 mm/s.
                        let speed = profile_number(&process, "outer_wall_speed")?
                            .max(100.0)
                            .min(profile_number(&filament, "filament_max_volumetric_speed")? / area)
                            .floor();
                        set(&mut process, "outer_wall_speed", speed);
                        set(&mut process, "inner_wall_speed", speed);
                        set(&mut process, "wall_loops", 2);
                        set(&mut process, "bottom_shell_layers", 0);
                        set(&mut process, "seam_position", "back");
                        set(&mut process, "brim_type", "brim_ears");
                        set(&mut process, "brim_object_gap", 0);
                        set(&mut process, "brim_ears_max_angle", 135);
                        set(&mut process, "brim_width", 6);
                        set(&mut filament, "adaptive_pressure_advance", json!(["0"]));
                        set(&mut filament, "slow_down_layer_time", json!(["1"]));
                    }
                    CalibrationStep::Retraction => {
                        let max_height = profile_nonnegative_number(&printer, "max_layer_height")?
                            .max(layer_height);
                        set(&mut printer, "max_layer_height", json!([max_height.to_string()]));
                        set(&mut process, "wall_loops", 2);
                        set(&mut process, "bottom_shell_layers", 3);
                        set(&mut printer, "use_relative_e_distances", true);
                        set(&mut printer, "use_firmware_retraction", false);
                        set(
                            &mut filament,
                            "filament_retraction_length",
                            json!([self.highest.to_string()]),
                        );
                        for key in [
                            "filament_wipe",
                            "filament_retract_restart_extra",
                            "filament_retraction_minimum_travel",
                        ] {
                            set(&mut filament, key, json!(["0"]));
                        }
                        set(&mut process, "reduce_crossing_wall", false);
                    }
                    CalibrationStep::VolumetricFlow => {
                        set(&mut printer, "use_relative_e_distances", true);
                        let span = volumetric_span.ok_or(AppError::Internal)?;
                        let (lo, hi) = mesh.bounds();
                        mesh.scale([((span - 10.0) / (hi[0] - lo[0])).min(1.0), 1.0, 1.0]);
                        let max_height = profile_nonnegative_number(&printer, "max_layer_height")?
                            .max(layer_height);
                        set(&mut printer, "max_layer_height", json!([max_height.to_string()]));
                        set(&mut filament, "filament_max_volumetric_speed", json!(["200"]));
                        set(&mut filament, "slow_down_layer_time", json!(["0"]));
                        set(&mut process, "wall_loops", 1);
                        set(&mut process, "bottom_shell_layers", 0);
                        set(&mut process, "outer_wall_line_width", nozzle * 1.75);
                        set(&mut process, "enable_overhang_speed", false);
                        set(&mut process, "overhang_reverse", false);
                        set(&mut process, "brim_type", "outer_and_inner");
                        set(&mut process, "brim_object_gap", 0);
                        set(&mut process, "brim_width", 5);
                        set(&mut process, "timelapse_type", "0");
                        set(&mut process, "spiral_mode", true);
                    }
                    _ => {}
                }
            }
            let name = Path::new(resource).file_stem().ok_or(AppError::Internal)?.to_string_lossy();
            let path = dir.join(format!("{name}.stl"));
            mesh.write(&path).await?;
            // CLI lacks set_calib_params. These expressions mirror GCode.cpp at the pinned commit.
            let expression = match self.step {
                CalibrationStep::Temperature => format!(
                    "int(({} + 5 - layer_num / (total_layer_count - 1.0) * ({} - {} + 5)) / 5) * 5",
                    self.highest, self.highest, self.lowest
                ),
                CalibrationStep::PressureAdvance => {
                    format!("{} + int(layer_z) * {}", self.lowest, self.increment)
                }
                CalibrationStep::Retraction => {
                    format!("{} + int(max(0, layer_z - 0.4)) * {}", self.lowest, self.increment)
                }
                CalibrationStep::VolumetricFlow => {
                    format!("{} + layer_z * {}", self.lowest, self.increment)
                }
                _ => unreachable!(),
            };
            let value = if self.step == CalibrationStep::Temperature {
                format!("{{if layer_num <= 1}}{}{{else}}{{{expression}}}{{endif}}", self.highest)
            } else {
                format!("{{{expression}}}")
            };
            let marker = format!(";LAYERCOVE_CALIBRATION {:?} {value} Z={{layer_z}}", self.step);
            let command = match self.step {
                CalibrationStep::Temperature => format!("M104 S{value}"),
                CalibrationStep::PressureAdvance => match flavor.as_str() {
                    "klipper" => format!("SET_PRESSURE_ADVANCE ADVANCE={{{expression}}}"),
                    "reprapfirmware" => format!("M572 D0 S{{{expression}}}"),
                    "bambu" => format!("M900 K{{{expression}}} L1000 M10"),
                    _ => format!("M900 K{{{expression}}}"),
                },
                _ => String::new(),
            };
            writeln!(&mut layer_code, "{marker}\n{command}").unwrap();
            if self.step == CalibrationStep::VolumetricFlow {
                let area = layer_height
                    * (nozzle * 1.75 - layer_height * (1.0 - std::f64::consts::PI / 4.0))
                    * profile_number(&filament, "filament_flow_ratio")?;
                let speed = (self.lowest + height * self.increment) / area;
                if speed
                    > profile_number(&printer, "machine_max_speed_x")?
                        .min(profile_number(&printer, "machine_max_speed_y")?)
                {
                    return Err(AppError::Bad(
                        "volumetric test exceeds printer XY speed limit".into(),
                    ));
                }
                writeln!(
                    &mut layer_code,
                    ";LAYERCOVE_CALIBRATION_SPEED {{if layer_num == 0}}{{initial_layer_speed}}{{else}}{{if slow_down_layers > 1 && layer_num < slow_down_layers}}{{min(round(({expression}) / {area}), initial_layer_speed + (round(({expression}) / {area}) - initial_layer_speed) * layer_num / slow_down_layers)}}{{else}}{{round(({expression}) / {area})}}{{endif}}{{endif}}"
                )
                .unwrap();
            }
            objects.push(json!({"path":path,"count":1,"filaments":[1]}));
        }
        let layer_code =
            format!("{}\n{layer_code}", printer["layer_change_gcode"].as_str().unwrap_or_default());
        set(&mut printer, "layer_change_gcode", layer_code);
        let end_code = format!(
            ";LAYERCOVE_CALIBRATION_END\n{}",
            printer["machine_end_gcode"].as_str().unwrap_or_default()
        );
        set(&mut printer, "machine_end_gcode", end_code);
        for (path, profile) in
            [(printer_path, printer), (process_path, process), (filament_path, filament)]
        {
            tokio::fs::write(path, serde_json::to_vec(&profile)?).await?;
        }
        let assembly = dir.join("calibration-assembly.json");
        tokio::fs::write(&assembly, serde_json::to_vec(&json!({"plates":[{"plate_name":"Calibration","need_arrange":self.step != CalibrationStep::FlowRate,"objects":objects}]}))?).await?;
        tokio::fs::write(dir.join("calibration.json"), serde_json::to_vec(self)?).await?;
        Ok(assembly)
    }
}

pub fn pressure_command(flavor: &str, value: f64) -> String {
    match flavor {
        "klipper" => format!("SET_PRESSURE_ADVANCE ADVANCE={value}"),
        "reprapfirmware" => format!("M572 D0 S{value}"),
        "bambu" => format!("M900 K{value} L1000 M10"),
        _ => format!("M900 K{value}"),
    }
}

/// Only generated relative-E, paired retract/recover moves may change. Custom start/end code
/// and deposition moves stay byte-for-byte unchanged. Unsupported extrusion fails closed.
pub fn rewrite_retraction(input: &str, request: &Calibration) -> Result<String, AppError> {
    let values = request.values()?;
    let movement =
        regex::Regex::new(r"^(G[01]) E(-?(?:\d+(?:\.\d*)?|\.\d+))((?: F\d+(?:\.\d*)?)?)$")
            .map_err(|_| AppError::Internal)?;
    let mut relative = false;
    let mut height = None;
    let mut startup_retraction = None;
    if values.iter().map(|value| decimal(*value)).collect::<std::collections::BTreeSet<_>>().len()
        != values.len()
    {
        return Err(AppError::Bad(
            "retraction values must remain distinct at Orca's five-decimal extrusion precision"
                .into(),
        ));
    }
    let mut pending: Option<(f64, f64, usize, String, usize)> = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut output = String::with_capacity(input.len());
    let mut ended = false;
    for line in input.lines() {
        let command = line.split(';').next().unwrap_or_default().trim();
        if command == "M83" {
            relative = true;
        }
        if command == "M82" {
            relative = false;
        }
        if line == ";LAYERCOVE_CALIBRATION_END" {
            ended = true;
            // Orca's final retract deliberately has no recovery. Keep that shutdown move intact.
            if let Some((_, _, position, original, _)) = pending.take() {
                let end =
                    output[position..].find('\n').map_or(output.len(), |offset| position + offset);
                output.replace_range(position..end, &original);
            }
        }
        let layer_z = line.strip_prefix("; Z_HEIGHT: ").or_else(|| {
            line.strip_prefix(";LAYERCOVE_CALIBRATION Retraction ")
                .and_then(|marker| marker.rsplit_once(" Z=").map(|(_, z)| z))
        });
        if let Some(z) = layer_z {
            height = Some(
                z.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| {
                        AppError::Execution("invalid calibration layer height".into())
                    })?,
            );
        }
        let rewritten = if !ended && let Some(z) = height {
            if let Some(captures) = movement.captures(command) {
                if !relative {
                    return Err(AppError::Bad(
                        "retraction calibration requires relative extrusion throughout the model"
                            .into(),
                    ));
                }
                let amount: f64 = captures[2].parse().map_err(|_| AppError::Internal)?;
                if amount < 0.0 {
                    if pending.is_some() {
                        return Err(AppError::Bad("unpaired calibration retraction".into()));
                    }
                    if decimal(-amount) != decimal(request.highest) {
                        return Err(AppError::Bad(
                            "custom extrusion moves are unsupported in retraction calibration"
                                .into(),
                        ));
                    }
                    let band = values
                        .iter()
                        .enumerate()
                        .rfind(|(i, _)| z >= 0.4 + *i as f64)
                        .map_or(0, |(i, _)| i);
                    let value = request.lowest + (z - 0.4).max(0.0).floor() * request.increment;
                    pending = Some((-amount, value, output.len(), line.to_owned(), band));
                    Some(format!("{} E-{}{}", &captures[1], decimal(value), &captures[3]))
                } else if let Some((retracted, value, _, _, band)) = pending.take() {
                    if amount != retracted {
                        return Err(AppError::Bad(
                            "calibration recovery must match the retraction".into(),
                        ));
                    }
                    seen.insert(band);
                    Some(format!("{} E{}{}", &captures[1], decimal(value), &captures[3]))
                } else if amount > 0.0 {
                    if startup_retraction.take() == Some(amount) {
                        None
                    } else {
                        return Err(AppError::Bad(format!(
                            "unpaired calibration extrusion: {line}"
                        )));
                    }
                } else {
                    None
                }
            } else {
                if pending.is_some()
                    && matches!(command.split_whitespace().next(), Some("G0" | "G1" | "G2" | "G3"))
                    && command.split_whitespace().any(|part| part.starts_with('E'))
                {
                    return Err(AppError::Bad(
                        "calibration recovery cannot be combined with a deposition move".into(),
                    ));
                }
                None
            }
        } else {
            if !ended && let Some(captures) = movement.captures(command) {
                let amount: f64 = captures[2].parse().map_err(|_| AppError::Internal)?;
                startup_retraction = if relative && amount < 0.0 { Some(-amount) } else { None };
            }
            None
        };
        writeln!(output, "{}", rewritten.as_deref().unwrap_or(line)).unwrap();
    }
    if !ended || pending.is_some() || seen.len() != values.len() {
        return Err(AppError::Execution(format!(
            "retraction artifact does not exercise every requested value with paired retracts: end={ended}, values={seen:?}, expected={}, pending={}",
            values.len(),
            pending.is_some()
        )));
    }
    Ok(output)
}

fn rewrite_volumetric(input: &str) -> Result<String, AppError> {
    let mut output = String::with_capacity(input.len());
    let mut speed = None;
    let mut wall = false;
    let mut changed = false;
    for line in input.lines() {
        if line == ";LAYERCOVE_CALIBRATION_END" {
            speed = None;
        }
        if let Some(value) = line.strip_prefix(";LAYERCOVE_CALIBRATION_SPEED ") {
            let value: f64 = value.parse().map_err(|_| AppError::Internal)?;
            if !value.is_finite() || value <= 0.0 {
                return Err(AppError::Execution("invalid volumetric speed marker".into()));
            }
            speed = Some(value * 60.0);
        }
        if line.starts_with("; FEATURE:") || line.starts_with(";TYPE:") {
            wall = line.contains("Outer wall");
        }
        let (code, comment) = line.split_once(';').unwrap_or((line, ""));
        if wall
            && let Some(speed) = speed
            && code.starts_with("G1 ")
            && code.split_whitespace().any(|s| {
                s.strip_prefix('E').and_then(|v| v.parse::<f64>().ok()).is_some_and(|v| v > 0.0)
            })
            && code.split_whitespace().any(|s| s.starts_with('X') || s.starts_with('Y'))
        {
            let parts: Vec<_> = code.split_whitespace().filter(|s| !s.starts_with('F')).collect();
            writeln!(&mut output, "{} F{speed:.3} ;{comment}", parts.join(" ")).unwrap();
            changed = true;
        } else {
            writeln!(&mut output, "{line}").unwrap();
        }
    }
    if !changed {
        return Err(AppError::Execution(
            "volumetric artifact has no recognized outer walls".into(),
        ));
    }
    Ok(output)
}

fn decimal(value: f64) -> String {
    // Orca's extrusion writer uses five decimal places.
    format!("{value:.5}").trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub async fn finish_artifact(
    bytes: Vec<u8>,
    export: bool,
    dir: &Path,
    request: &Calibration,
) -> Result<Vec<u8>, AppError> {
    if !matches!(request.step, CalibrationStep::Retraction | CalibrationStep::VolumetricFlow) {
        return Ok(bytes);
    }
    let rewrite = |text: &str| {
        if request.step == CalibrationStep::Retraction {
            rewrite_retraction(text, request)
        } else {
            rewrite_volumetric(text)
        }
    };
    if !export {
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| AppError::Execution("calibration G-code is not UTF-8".into()))?;
        return Ok(rewrite(text)?.into_bytes());
    }
    use std::io::{Cursor, Read, Write as IoWrite};
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes))
        .map_err(|error| AppError::Execution(error.to_string()))?;
    let gcode_entries = archive
        .file_names()
        .filter(|name| name.ends_with(".gcode"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if gcode_entries.len() != 1 {
        return Err(AppError::Execution("calibration requires one 3MF plate".into()));
    }
    let name = &gcode_entries[0];
    let mut gcode = String::new();
    {
        let mut entry =
            archive.by_name(name).map_err(|error| AppError::Execution(error.to_string()))?;
        if entry.size() > 512 * 1024 * 1024 {
            return Err(AppError::Execution("3MF plate G-code exceeds 512MiB".into()));
        }
        entry.read_to_string(&mut gcode)?;
    }
    let rewritten = rewrite(&gcode)?;
    let digest_path = dir.join("retraction.gcode");
    tokio::fs::write(&digest_path, &rewritten).await?;
    // Coreutils is present in the pinned runtime. Orca/Bambu requires an MD5 sidecar for plate G-code.
    let digest = tokio::process::Command::new("md5sum").arg(&digest_path).output().await?;
    let digest = std::str::from_utf8(&digest.stdout)
        .ok()
        .and_then(|text| text.split_whitespace().next())
        .filter(|value| {
            digest.status.success()
                && value.len() == 32
                && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| AppError::Execution("could not checksum calibration plate".into()))?;
    let checksum_name = format!("{name}.md5");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut checksum_written = false;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|error| AppError::Execution(error.to_string()))?;
        if entry.name() == name {
            writer
                .start_file(name, options)
                .map_err(|error| AppError::Execution(error.to_string()))?;
            writer.write_all(rewritten.as_bytes())?;
        } else if entry.name() == checksum_name {
            writer
                .start_file(&checksum_name, options)
                .map_err(|error| AppError::Execution(error.to_string()))?;
            writer.write_all(digest.as_bytes())?;
            checksum_written = true;
        } else {
            writer.raw_copy_file(entry).map_err(|error| AppError::Execution(error.to_string()))?;
        }
    }
    if !checksum_written {
        writer
            .start_file(&checksum_name, options)
            .map_err(|error| AppError::Execution(error.to_string()))?;
        writer.write_all(digest.as_bytes())?;
    }
    Ok(writer.finish().map_err(|error| AppError::Execution(error.to_string()))?.into_inner())
}

fn set(profile: &mut Value, key: &str, value: impl Into<Value>) {
    // Orca's JSON profile loader accepts scalar settings as strings.
    profile[key] = match value.into() {
        Value::Bool(value) => if value { "1" } else { "0" }.into(),
        Value::Number(value) => value.to_string().into(),
        value => value,
    };
}
fn bed_bounds(profile: &Value) -> Result<([f64; 2], [f64; 2]), AppError> {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    let points = profile["printable_area"]
        .as_array()
        .ok_or_else(|| AppError::Bad("printable_area is required".into()))?;
    for point in points {
        let (x, y) = point
            .as_str()
            .and_then(|s| s.split_once('x'))
            .ok_or_else(|| AppError::Bad("invalid printable_area".into()))?;
        let values = [x.parse::<f64>(), y.parse::<f64>()];
        for (i, value) in values.into_iter().enumerate() {
            let value = value
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| AppError::Bad("invalid printable_area".into()))?;
            low[i] = low[i].min(value);
            high[i] = high[i].max(value);
        }
    }
    if (0..2).any(|i| !low[i].is_finite() || !high[i].is_finite() || high[i] <= low[i]) {
        return Err(AppError::Bad("empty printable_area".into()));
    }
    Ok((low, high))
}
fn volumetric_bed_span(printer: &Value) -> Result<f64, AppError> {
    let (low, high) = bed_bounds(printer)?;
    let span = high[0] - low[0];
    if !span.is_finite() || span <= 10.0 {
        return Err(AppError::Bad("volumetric bed must fit the 10 mm brim allowance".into()));
    }
    Ok(span)
}
pub fn profile_number(profile: &Value, key: &str) -> Result<f64, AppError> {
    let number = profile_nonnegative_number(profile, key)?;
    if number == 0.0 {
        return Err(AppError::Bad(format!("positive profile setting required: {key}")));
    }
    Ok(number)
}
fn profile_nonnegative_number(profile: &Value, key: &str) -> Result<f64, AppError> {
    let value = profile[key].as_array().and_then(|a| a.first()).unwrap_or(&profile[key]);
    let number = value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()));
    number
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| AppError::Bad(format!("nonnegative profile setting required: {key}")))
}
fn set_result(profile: &mut Value, step: CalibrationStep, value: f64) {
    let key = match step {
        CalibrationStep::Temperature => "nozzle_temperature",
        CalibrationStep::FlowRate => "filament_flow_ratio",
        CalibrationStep::PressureAdvance => "pressure_advance",
        CalibrationStep::Retraction => "filament_retraction_length",
        CalibrationStep::VolumetricFlow => "filament_max_volumetric_speed",
    };
    set(profile, key, json!([value.to_string()]));
    if step == CalibrationStep::Temperature {
        set(profile, "nozzle_temperature_initial_layer", json!([value.to_string()]));
    }
    if step == CalibrationStep::PressureAdvance {
        set(profile, "enable_pressure_advance", json!(["1"]));
    }
}

#[cfg(test)]
mod volumetric_tests {
    use super::*;
    #[test]
    fn malformed_and_unusable_beds_are_bad_requests() {
        for area in [
            json!([]),
            json!(["bad"]),
            json!(["NaNx0", "250x250"]),
            json!(["0x0", "0x250"]),
            json!(["0x0", "5x250"]),
            json!(["0x0", "10x250"]),
        ] {
            assert!(matches!(
                volumetric_bed_span(&json!({"printable_area":area})),
                Err(AppError::Bad(_))
            ));
        }
        assert_eq!(
            volumetric_bed_span(&json!({"printable_area":["-10x-10","240x240"]})).unwrap(),
            250.0
        );
    }
    #[test]
    fn only_positive_extrusion_wall_moves_receive_the_layer_speed() {
        let input = ";LAYERCOVE_CALIBRATION_SPEED 20\n;TYPE:Outer wall\nG1 X1 Y2 E-.01 F900\nG1 X2 E0 F900\nG1 X3 E.1 F900\n;LAYERCOVE_CALIBRATION_SPEED 30\nG1 X4 E.1\n;LAYERCOVE_CALIBRATION_END\nG1 X5 E.1 F900\n";
        let result = rewrite_volumetric(input).unwrap();
        assert!(result.contains("G1 X1 Y2 E-.01 F900\n"));
        assert!(result.contains("G1 X2 E0 F900\n"));
        assert!(result.contains("G1 X3 E.1 F1200.000"));
        assert!(result.contains("G1 X4 E.1 F1800.000"));
        assert!(result.ends_with("G1 X5 E.1 F900\n"));
    }
}
