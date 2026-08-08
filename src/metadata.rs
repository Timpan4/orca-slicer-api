use std::io::{Cursor, Read};

#[derive(Debug, PartialEq)]
pub struct SliceMetadata {
    pub print_time_seconds: u64,
    pub filament_used_g: f64,
    pub filament_used_mm: f64,
}

pub fn from_artifact(
    bytes: &[u8],
    extension: Option<&str>,
    plate: Option<u32>,
) -> Result<SliceMetadata, String> {
    match extension {
        Some("gcode") => parse_gcode(bytes),
        Some("3mf") => metadata_from_3mf(bytes, plate),
        _ => Err("unsupported slicer output metadata format".into()),
    }
}

fn metadata_from_3mf(bytes: &[u8], plate: Option<u32>) -> Result<SliceMetadata, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut candidates = Vec::new();
    for index in 0..archive.len() {
        let name = archive.by_index(index).map_err(|error| error.to_string())?.name().to_owned();
        if name.to_ascii_lowercase().ends_with(".gcode") {
            candidates.push((index, name));
        }
    }
    if plate == Some(0) {
        if candidates.is_empty() {
            return Err("3MF does not contain plate G-code".into());
        }
        let mut total =
            SliceMetadata { print_time_seconds: 0, filament_used_g: 0.0, filament_used_mm: 0.0 };
        for (index, _) in candidates {
            let metadata = parse_gcode(&read_gcode(&mut archive, index)?)?;
            total.print_time_seconds = total
                .print_time_seconds
                .checked_add(metadata.print_time_seconds)
                .ok_or("combined plate print time exceeds supported range")?;
            total.filament_used_g += metadata.filament_used_g;
            total.filament_used_mm += metadata.filament_used_mm;
        }
        return Ok(total);
    }
    let preferred = plate.and_then(|plate| {
        let marker = format!("plate_{plate}.gcode");
        candidates.iter().find(|(_, name)| name.to_ascii_lowercase().ends_with(&marker))
    });
    let index = preferred
        .map(|(index, _)| *index)
        .or_else(|| (candidates.len() == 1).then(|| candidates[0].0))
        .ok_or("3MF does not contain one unambiguous plate G-code")?;
    parse_gcode(&read_gcode(&mut archive, index)?)
}

fn read_gcode(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    index: usize,
) -> Result<Vec<u8>, String> {
    let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
    if entry.size() > 512 * 1024 * 1024 {
        return Err("3MF plate G-code exceeds 512MiB".into());
    }
    let mut gcode = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry.read_to_end(&mut gcode).map_err(|error| error.to_string())?;
    Ok(gcode)
}

fn parse_gcode(bytes: &[u8]) -> Result<SliceMetadata, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut print_time = None;
    let mut filament_mm = None;
    let mut filament_g = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("; estimated printing time (normal mode) = ") {
            print_time = parse_duration(value);
        } else if let Some((_, total)) = line
            .strip_prefix("; model printing time: ")
            .and_then(|value| value.split_once("; total estimated time: "))
        {
            print_time = parse_duration(total);
        } else if let Some(value) = line.strip_prefix("; filament used [mm] = ") {
            filament_mm = parse_number_list(value);
        } else if let Some(value) = line.strip_prefix("; total filament used [g] = ") {
            filament_g = parse_number(value.trim());
        } else if filament_g.is_none()
            && let Some(value) = line.strip_prefix("; filament used [g] = ")
        {
            filament_g = parse_number_list(value);
        }
    }
    Ok(SliceMetadata {
        print_time_seconds: print_time.ok_or("G-code missing estimated print time")?,
        filament_used_g: filament_g.ok_or("G-code missing filament weight")?,
        filament_used_mm: filament_mm.ok_or("G-code missing filament length")?,
    })
}

fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|number| number.is_finite() && *number >= 0.0)
}

fn parse_number_list(value: &str) -> Option<f64> {
    value.split(',').map(|part| parse_number(part.trim())).try_fold(0.0, |sum, number| {
        number.and_then(|number| (sum + number).is_finite().then_some(sum + number))
    })
}

fn parse_duration(value: &str) -> Option<u64> {
    let mut total = 0_u64;
    let mut found = false;
    for part in value.split_whitespace() {
        let (number, multiplier) = match part.as_bytes().last().copied()? {
            b'd' => (&part[..part.len() - 1], 86_400),
            b'h' => (&part[..part.len() - 1], 3_600),
            b'm' => (&part[..part.len() - 1], 60),
            b's' => (&part[..part.len() - 1], 1),
            _ => return None,
        };
        total = total.checked_add(number.parse::<u64>().ok()?.checked_mul(multiplier)?)?;
        found = true;
    }
    found.then_some(total)
}
