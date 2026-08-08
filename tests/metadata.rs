use orca_slicer_api::metadata::{SliceMetadata, from_artifact};
use std::io::{Cursor, Write};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const GCODE: &str = "; estimated printing time (normal mode) = 1h 2m 3s\n; filament used [mm] = 100.5, 2.5\n; total filament used [g] = 3.25\n";

#[test]
fn metadata_is_read_from_gcode_and_selected_3mf_plate() {
    let expected =
        SliceMetadata { print_time_seconds: 3_723, filament_used_g: 3.25, filament_used_mm: 103.0 };
    assert_eq!(from_artifact(GCODE.as_bytes(), Some("gcode"), None).unwrap(), expected);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer.start_file("Metadata/plate_1.gcode", options).unwrap();
    writer
        .write_all(b"; estimated printing time (normal mode) = 1s\n; filament used [mm] = 1\n; total filament used [g] = 1\n")
        .unwrap();
    writer.start_file("Metadata/plate_2.gcode", options).unwrap();
    writer.write_all(GCODE.as_bytes()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    assert_eq!(from_artifact(&bytes, Some("3mf"), Some(2)).unwrap(), expected);
    assert_eq!(
        from_artifact(&bytes, Some("3mf"), Some(0)).unwrap(),
        SliceMetadata { print_time_seconds: 3_724, filament_used_g: 4.25, filament_used_mm: 104.0 }
    );
    assert!(from_artifact(&bytes, Some("3mf"), None).unwrap_err().contains("unambiguous"));
}
