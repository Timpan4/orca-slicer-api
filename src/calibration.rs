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
    pub lowest: f64,
    pub highest: f64,
    pub increment: f64,
    pub baseline: f64,
    #[serde(default)]
    pub previous_results: std::collections::BTreeMap<CalibrationStep, f64>,
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
        let values = self.values()?;
        let mut printer: Value = serde_json::from_slice(&tokio::fs::read(printer_path).await?)?;
        let mut process: Value = serde_json::from_slice(&tokio::fs::read(process_path).await?)?;
        let mut filament: Value = serde_json::from_slice(&tokio::fs::read(filament_path).await?)?;
        let temperature = if self.step == CalibrationStep::Temperature {
            self.highest.max(self.baseline)
        } else {
            *self
                .previous_results
                .get(&CalibrationStep::Temperature)
                .ok_or_else(|| AppError::Bad("temperature result is required".into()))?
        };
        if temperature > profile_number(&filament, "nozzle_temperature_range_high")? {
            return Err(AppError::Bad(
                "temperature exceeds this filament profile's maximum".into(),
            ));
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
        let layer_height = nozzle / 2.0;
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
        set(&mut process, "layer_height", layer_height);
        set(&mut process, "initial_layer_print_height", layer_height);
        set(&mut process, "alternate_extra_wall", false);
        set(&mut process, "seam_slope_type", "none");
        set(&mut process, "enable_wrapping_detection", false);
        set(&mut process, "max_volumetric_extrusion_rate_slope", 0);
        set(&mut process, "brim_type", "outer_only");
        set(&mut process, "brim_object_gap", 0);
        set(&mut process, "enable_support", false);
        set(&mut process, "ironing_type", "no ironing");
        set(&mut process, "print_sequence", "by layer");
        set(&mut printer, "resonance_avoidance", false);
        let band =
            if self.step == CalibrationStep::Temperature { 10.0 * nozzle / 0.4 } else { 1.0 };
        let base = if self.step == CalibrationStep::Retraction { 0.4 } else { 0.0 };
        let height = base + values.len() as f64 * band;
        if self.step != CalibrationStep::FlowRate
            && height > profile_number(&printer, "printable_height")?
        {
            return Err(AppError::Bad(
                "calibration tower exceeds the printer's printable height".into(),
            ));
        }
        let mut objects = Vec::new();
        let mut layer_code = String::new();
        if self.step == CalibrationStep::FlowRate {
            set(&mut process, "wall_loops", 1);
            set(&mut process, "top_shell_layers", 5);
            set(&mut process, "bottom_shell_layers", 2);
            set(&mut process, "sparse_infill_density", "35%");
            set(&mut process, "top_surface_pattern", "monotonic");
            set(&mut process, "top_surface_line_width", nozzle * 1.2);
            for (i, value) in values.iter().enumerate() {
                let path = dir.join(format!("sample_{}_{}.stl", i + 1, value));
                let mut mesh = String::from("solid calibration\n");
                cuboid(&mut mesh, [0.0, 0.0, 0.0], [20.0, 25.0, layer_height * 10.0]);
                // A numbered tab identifies each tile independently of auto-arrangement.
                digits(&mut mesh, &(i + 1).to_string(), layer_height * 10.0);
                mesh.push_str("endsolid calibration\n");
                tokio::fs::write(&path, mesh).await?;
                objects.push(json!({"path":path,"count":1,"filaments":[1],"print_params":{"print_flow_ratio":(value / self.baseline).to_string()}}));
            }
        } else {
            let path = dir.join("calibration.stl");
            let mut mesh = String::from("solid calibration\n");
            match self.step {
                CalibrationStep::Temperature => {
                    for i in 0..values.len() {
                        let z = i as f64 * band;
                        cuboid(&mut mesh, [0.0, 0.0, z], [5.0, 15.0, band]);
                        cuboid(&mut mesh, [25.0, 0.0, z], [5.0, 15.0, band]);
                        cuboid(
                            &mut mesh,
                            [5.0, 0.0, z + band - layer_height * 3.0],
                            [20.0, 15.0, layer_height * 3.0],
                        );
                    }
                    set(
                        &mut filament,
                        "nozzle_temperature",
                        json!([values.last().unwrap().to_string()]),
                    );
                    set(
                        &mut filament,
                        "nozzle_temperature_initial_layer",
                        json!([values.last().unwrap().to_string()]),
                    );
                }
                CalibrationStep::Retraction => {
                    cuboid(&mut mesh, [0.0, 0.0, 0.0], [35.0, 10.0, base]);
                    cuboid(&mut mesh, [0.0, 0.0, base], [5.0, 10.0, height - base]);
                    cuboid(&mut mesh, [30.0, 0.0, base], [5.0, 10.0, height - base]);
                    // Force paired relative-E retracts so the shim can vary only these pairs.
                    set(&mut printer, "use_relative_e_distances", true);
                    set(&mut printer, "use_firmware_retraction", false);
                    set(
                        &mut filament,
                        "filament_retraction_length",
                        json!([self.highest.to_string()]),
                    );
                    set(&mut filament, "filament_wipe", json!(["0"]));
                    set(&mut filament, "filament_retract_restart_extra", json!(["0"]));
                    set(&mut filament, "filament_retraction_minimum_travel", json!(["0"]));
                    set(&mut process, "reduce_crossing_wall", false);
                }
                CalibrationStep::PressureAdvance => {
                    cuboid(&mut mesh, [0.0, 0.0, 0.0], [30.0, 30.0, height]);
                    set(&mut process, "seam_position", "back");
                    set(&mut filament, "adaptive_pressure_advance", json!(["0"]));
                    set(&mut filament, "slow_down_layer_time", json!(["0"]));
                }
                CalibrationStep::VolumetricFlow => {
                    cuboid(&mut mesh, [0.0, 0.0, 0.0], [100.0, 20.0, height]);
                    set(&mut process, "wall_loops", 1);
                    set(&mut process, "outer_wall_line_width", nozzle * 1.75);
                    set(
                        &mut filament,
                        "filament_max_volumetric_speed",
                        json!([self.highest.to_string()]),
                    );
                    set(&mut filament, "slow_down_layer_time", json!(["0"]));
                    set(&mut process, "enable_overhang_speed", false);
                }
                CalibrationStep::FlowRate => unreachable!(),
            }
            mesh.push_str("endsolid calibration\n");
            tokio::fs::write(&path, mesh).await?;
            if self.step != CalibrationStep::Temperature {
                set(&mut process, "top_shell_layers", 0);
                set(&mut process, "bottom_shell_layers", 2);
                set(&mut process, "sparse_infill_density", "0%");
            }
            let mut ranges = Vec::new();
            for (i, value) in values.iter().enumerate() {
                let low = if i == 0 { 0.0 } else { base + i as f64 * band };
                let high = base + (i + 1) as f64 * band;
                let actual = if self.step == CalibrationStep::Temperature {
                    values[values.len() - 1 - i]
                } else {
                    *value
                };
                let command = match self.step {
                    CalibrationStep::Temperature => format!("M104 S{actual}"),
                    CalibrationStep::PressureAdvance => pressure_command(&flavor, actual),
                    _ => String::new(),
                };
                writeln!(&mut layer_code, "{{if layer_z >= {low} and layer_z < {high}}}\n;LAYERCOVE_CALIBRATION {:?} {actual} Z={{layer_z}}\n{command}\n{{endif}}", self.step).unwrap();
                if self.step == CalibrationStep::VolumetricFlow {
                    let width = nozzle * 1.75;
                    // Orca Flow::mm3_per_mm, rounded-rectangle bead cross section.
                    let area =
                        layer_height * (width - layer_height * (1.0 - std::f64::consts::PI / 4.0));
                    let flow = profile_number(&filament, "filament_flow_ratio")?;
                    let speed = actual / (area * flow);
                    if speed
                        > profile_number(&printer, "machine_max_speed_x")?
                            .min(profile_number(&printer, "machine_max_speed_y")?)
                    {
                        return Err(AppError::Bad(
                            "volumetric test exceeds the printer profile's XY speed limit".into(),
                        ));
                    }
                    ranges.push(json!({"min_z":low,"max_z":high,"range_params":{"layer_height":layer_height.to_string(),"outer_wall_speed":speed.to_string(),"inner_wall_speed":speed.to_string()}}));
                }
            }
            objects.push(json!({"path":path,"count":1,"filaments":[1],"height_ranges":ranges}));
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
        tokio::fs::write(&assembly, serde_json::to_vec(&json!({"plates":[{"plate_name":"Calibration","need_arrange":true,"objects":objects}]}))?).await?;
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
                    let value = values[band];
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
    if request.step != CalibrationStep::Retraction {
        return Ok(bytes);
    }
    if !export {
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| AppError::Execution("calibration G-code is not UTF-8".into()))?;
        return Ok(rewrite_retraction(text, request)?.into_bytes());
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
    let rewritten = rewrite_retraction(&gcode, request)?;
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
    profile[key] = value.into();
}
pub fn profile_number(profile: &Value, key: &str) -> Result<f64, AppError> {
    let value = profile[key].as_array().and_then(|a| a.first()).unwrap_or(&profile[key]);
    let number = value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()));
    number
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| AppError::Bad(format!("positive profile setting required: {key}")))
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

fn cuboid(out: &mut String, origin: [f64; 3], size: [f64; 3]) {
    let [x, y, z] = origin;
    let [w, d, h] = size;
    let v = [
        [x, y, z],
        [x + w, y, z],
        [x + w, y + d, z],
        [x, y + d, z],
        [x, y, z + h],
        [x + w, y, z + h],
        [x + w, y + d, z + h],
        [x, y + d, z + h],
    ];
    for face in [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ] {
        out.push_str("facet normal 0 0 0\nouter loop\n");
        for index in face {
            let [a, b, c] = v[index];
            writeln!(out, "vertex {a} {b} {c}").unwrap();
        }
        out.push_str("endloop\nendfacet\n");
    }
}
fn digits(out: &mut String, text: &str, z: f64) {
    const FONT: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    for (i, digit) in text.bytes().enumerate() {
        for (row, bits) in FONT[usize::from(digit - b'0')].iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) != 0 {
                    cuboid(
                        out,
                        [1.0 + i as f64 * 4.0 + f64::from(col), 5.0 - row as f64, z],
                        [1.0, 1.0, 0.4],
                    );
                }
            }
        }
    }
}
