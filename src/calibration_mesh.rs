//! Trusted Orca resource loading and horizontal cuts. Request data never selects paths.
use crate::error::AppError;
use std::{collections::BTreeMap, io::Read, path::Path};
#[cfg(test)]
#[path = "calibration_mesh_tests.rs"]
mod tests;
pub type Point = [f64; 3];
pub struct Mesh(pub Vec<[Point; 3]>);
fn resource_error(error: impl std::fmt::Display) -> AppError {
    AppError::Execution(format!("Orca calibration resource: {error}"))
}
impl Mesh {
    pub async fn tower(dir: &Path, resource: &str, cli: &str) -> Result<Self, AppError> {
        let root = std::env::var("ORCA_CALIBRATION_PATH")
            .unwrap_or_else(|_| "/app/orca/resources/calib".into());
        let mut command = tokio::process::Command::new(cli);
        command
            .args(["--export-stl", "--outputdir"])
            .arg(dir)
            .arg(Path::new(&root).join(resource))
            .current_dir(dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let (child, mut group) = crate::slice::spawn_process(&mut command)?;
        let output = tokio::time::timeout(crate::slice::SLICE_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| resource_error("export exceeded slicer timeout"))??;
        group.terminate();
        if !output.status.success() {
            return Err(resource_error(String::from_utf8_lossy(&output.stderr)));
        }
        let name = Path::new(resource).file_name().ok_or(AppError::Internal)?.to_string_lossy();
        let bytes = tokio::fs::read(dir.join("stl").join(format!("obj_1_{name}.stl"))).await?;
        Self::stl(&bytes)
    }
    pub fn stl(bytes: &[u8]) -> Result<Self, AppError> {
        if bytes.len() < 84 {
            return Err(resource_error("truncated STL"));
        }
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        if bytes.len() != 84 + count * 50 {
            return Err(resource_error("invalid STL length"));
        }
        let triangles = bytes[84..]
            .chunks_exact(50)
            .map(|face| {
                std::array::from_fn(|i| {
                    std::array::from_fn(|j| {
                        let start = 12 + i * 12 + j * 4;
                        f64::from(f32::from_le_bytes(face[start..start + 4].try_into().unwrap()))
                    })
                })
            })
            .collect();
        Ok(Self(triangles))
    }
    pub fn bounds(&self) -> (Point, Point) {
        let mut low = [f64::INFINITY; 3];
        let mut high = [f64::NEG_INFINITY; 3];
        for p in self.0.iter().flatten() {
            for i in 0..3 {
                low[i] = low[i].min(p[i]);
                high[i] = high[i].max(p[i]);
            }
        }
        (low, high)
    }
    pub fn scale(&mut self, scale: Point) {
        for p in self.0.iter_mut().flatten() {
            for i in 0..3 {
                p[i] *= scale[i];
            }
        }
    }
    pub fn on_bed(&mut self) {
        let z = self.bounds().0[2];
        for p in self.0.iter_mut().flatten() {
            p[2] -= z;
        }
    }
    pub async fn write(&self, path: &Path) -> Result<(), AppError> {
        let mut bytes = vec![0; 80];
        bytes
            .extend_from_slice(&u32::try_from(self.0.len()).map_err(resource_error)?.to_le_bytes());
        for face in &self.0 {
            bytes.extend_from_slice(&[0; 12]);
            for p in face {
                for v in p {
                    bytes.extend_from_slice(&(*v as f32).to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0; 2]);
        }
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }
    pub fn cut(&mut self, z: f64, lower: bool) -> Result<(), AppError> {
        // STL stores f32 coordinates. Weld at that precision before constructing
        // topology, so serialization cannot collapse distinct cut vertices.
        let z = f64::from(z as f32);
        for p in self.0.iter_mut().flatten() {
            *p = p.map(|v| f64::from(v as f32));
        }
        let inside = |p: Point| if lower { p[2] <= z } else { p[2] >= z };
        let mut faces = Vec::new();
        let mut edges = BTreeMap::new();
        for face in &self.0 {
            let mut polygon = Vec::new();
            for i in 0..3 {
                let a = face[i];
                let b = face[(i + 1) % 3];
                if inside(a) {
                    polygon.push(a);
                }
                if inside(a) != inside(b) {
                    // Canonical edge order gives identical intersections on adjacent faces.
                    let (a, b) = if key(a) < key(b) { (a, b) } else { (b, a) };
                    let t = (z - a[2]) / (b[2] - a[2]);
                    let p = if a[2] == z {
                        a
                    } else if b[2] == z {
                        b
                    } else {
                        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), z]
                    };
                    polygon.push(p.map(|v| f64::from(v as f32)));
                }
            }
            polygon.dedup();
            if polygon.len() > 1 && polygon.first() == polygon.last() {
                polygon.pop();
            }
            for i in 1..polygon.len().saturating_sub(1) {
                let face = [polygon[0], polygon[i], polygon[i + 1]];
                if nonzero(&face) {
                    faces.push(face);
                }
            }
        }
        // Only unmatched edges of retained faces need caps. This also handles cuts
        // through existing vertices and edges without duplicating contour segments.
        let mut boundary = BTreeMap::new();
        for face in &faces {
            for i in 0..3 {
                let (a, b) = (face[i], face[(i + 1) % 3]);
                if a[2] == z && b[2] == z {
                    let (a, b) = if key(a) < key(b) { (key(a), key(b)) } else { (key(b), key(a)) };
                    *boundary.entry((a, b)).or_insert(0) += 1;
                }
            }
        }
        for ((a, b), count) in boundary {
            if count == 1 {
                edges.entry(a).or_insert_with(Vec::new).push(point(b));
                edges.entry(b).or_insert_with(Vec::new).push(point(a));
            } else if count != 2 {
                return Err(resource_error("non-manifold cut boundary"));
            }
        }
        let mut loops = Vec::new();
        while let Some((&start, _)) = edges.first_key_value() {
            let mut contour = Vec::new();
            let mut current = start;
            let mut previous = None;
            loop {
                let adjacent =
                    edges.remove(&current).ok_or_else(|| resource_error("open cut contour"))?;
                contour.push(point(current));
                let next = adjacent
                    .iter()
                    .find(|p| Some(key(**p)) != previous)
                    .ok_or_else(|| resource_error("broken cut contour"))?;
                let next = key(*next);
                if next == start {
                    break;
                }
                previous = Some(current);
                current = next;
            }
            loops.push(contour);
        }
        // Each even-depth contour is an outside boundary; its direct children are holes.
        let depths: Vec<_> = loops
            .iter()
            .enumerate()
            .map(|(i, a)| {
                loops.iter().enumerate().filter(|(j, b)| i != *j && contains(b, a[0])).count()
            })
            .collect();
        for (i, outer) in loops.iter().enumerate().filter(|(i, _)| depths[*i] % 2 == 0) {
            let mut points = outer.clone();
            let mut holes = Vec::new();
            for (j, hole) in loops.iter().enumerate() {
                if depths[j] == depths[i] + 1 && contains(outer, hole[0]) {
                    holes.push(points.len());
                    points.extend(hole);
                }
            }
            let coords: Vec<_> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
            let indices = earcutr::earcut(&coords, &holes, 2).map_err(resource_error)?;
            for t in indices.chunks_exact(3) {
                let mut face = [points[t[0]], points[t[1]], points[t[2]]];
                let area = (face[1][0] - face[0][0]) * (face[2][1] - face[0][1])
                    - (face[1][1] - face[0][1]) * (face[2][0] - face[0][0]);
                if (area > 0.0) != lower {
                    face.swap(1, 2);
                }
                if nonzero(&face) {
                    // Earcut can omit collinear contour vertices. Split its boundary
                    // edges at those vertices to retain the side mesh's exact edges.
                    let mut pending = vec![face];
                    while let Some(face) = pending.pop() {
                        let split = (0..3).find_map(|i| {
                            let a = face[i];
                            let b = face[(i + 1) % 3];
                            points
                                .iter()
                                .find(|p| **p != a && **p != b && on_segment(a, b, **p))
                                .map(|p| (i, *p))
                        });
                        if let Some((i, p)) = split {
                            let a = face[i];
                            let b = face[(i + 1) % 3];
                            let c = face[(i + 2) % 3];
                            pending.push([a, p, c]);
                            pending.push([p, b, c]);
                        } else {
                            faces.push(face);
                        }
                    }
                }
            }
        }
        self.0 = faces;
        Ok(())
    }
}
fn nonzero(f: &[Point; 3]) -> bool {
    let a = std::array::from_fn::<_, 3, _>(|i| f[1][i] - f[0][i]);
    let b = std::array::from_fn::<_, 3, _>(|i| f[2][i] - f[0][i]);
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
        .iter()
        .any(|v| *v != 0.0)
}
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let px = p[0] - a[0];
    let py = p[1] - a[1];
    let cross = dx * py - dy * px;
    let dot = px * dx + py * dy;
    // Roundoff bound for the determinant; coordinates remain f64 until STL export.
    cross.abs() <= f64::EPSILON * (dx.abs() * py.abs() + dy.abs() * px.abs())
        && dot > 0.0
        && dot < dx * dx + dy * dy
}
fn key(p: Point) -> [u64; 3] {
    p.map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}
fn point(p: [u64; 3]) -> Point {
    p.map(f64::from_bits)
}
fn contains(poly: &[Point], p: Point) -> bool {
    let mut result = false;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            result = !result;
        }
    }
    result
}
pub fn flow_models(path: &Path) -> Result<Vec<(String, Mesh)>, AppError> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(resource_error)?;
    let mut xml = String::new();
    archive.by_name("3D/3dmodel.model").map_err(resource_error)?.read_to_string(&mut xml)?;
    flow_xml(&xml)
}
fn flow_xml(xml: &str) -> Result<Vec<(String, Mesh)>, AppError> {
    let doc = roxmltree::Document::parse(xml).map_err(resource_error)?;
    let mut objects = Vec::new();
    for object in doc.descendants().filter(|n| n.has_tag_name("object")) {
        let name = object
            .attribute("name")
            .ok_or_else(|| resource_error("missing flow object name"))?
            .to_owned();
        let vertices: Vec<Point> = object
            .descendants()
            .filter(|n| n.has_tag_name("vertex"))
            .map(|v| {
                let coordinate = |k| {
                    v.attribute(k)
                        .ok_or_else(|| resource_error("missing vertex"))?
                        .parse()
                        .map_err(resource_error)
                };
                Ok([coordinate("x")?, coordinate("y")?, coordinate("z")?])
            })
            .collect::<Result<_, AppError>>()?;
        let triangles: Vec<_> = object
            .descendants()
            .filter(|n| n.has_tag_name("triangle"))
            .map(|t| {
                let vertex = |k| -> Result<Point, AppError> {
                    let index: usize = t
                        .attribute(k)
                        .ok_or_else(|| resource_error("missing triangle"))?
                        .parse()
                        .map_err(resource_error)?;
                    vertices.get(index).copied().ok_or_else(|| resource_error("invalid triangle"))
                };
                Ok([vertex("v1")?, vertex("v2")?, vertex("v3")?])
            })
            .collect::<Result<_, AppError>>()?;
        if triangles.is_empty() {
            return Err(resource_error("unsupported flow resource components"));
        }
        let id = object.attribute("id").ok_or_else(|| resource_error("missing object id"))?;
        objects.push((id, name, Mesh(triangles)));
    }
    let mut instances = Vec::new();
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let id =
            item.attribute("objectid").ok_or_else(|| resource_error("missing build object id"))?;
        let (_, name, mesh) = objects
            .iter()
            .find(|(object_id, _, _)| *object_id == id)
            .ok_or_else(|| resource_error("unknown build object"))?;
        let mut triangles = mesh.0.clone();
        if let Some(text) = item.attribute("transform") {
            let matrix = text
                .split_whitespace()
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(resource_error)?;
            if matrix.len() != 12 || matrix.iter().any(|v| !v.is_finite()) {
                return Err(resource_error("unsupported build transform"));
            }
            let determinant = matrix[0] * (matrix[4] * matrix[8] - matrix[5] * matrix[7])
                - matrix[1] * (matrix[3] * matrix[8] - matrix[5] * matrix[6])
                + matrix[2] * (matrix[3] * matrix[7] - matrix[4] * matrix[6]);
            if determinant == 0.0 || !determinant.is_finite() {
                return Err(resource_error("singular build transform"));
            }
            for face in &mut triangles {
                for p in face.iter_mut() {
                    *p = std::array::from_fn(|i| {
                        p[0] * matrix[i]
                            + p[1] * matrix[3 + i]
                            + p[2] * matrix[6 + i]
                            + matrix[9 + i]
                    });
                    if p.iter().any(|v| !v.is_finite()) {
                        return Err(resource_error("non-finite transformed vertex"));
                    }
                }
                if determinant < 0.0 {
                    face.swap(1, 2);
                }
            }
        }
        instances.push((name.clone(), Mesh(triangles)));
    }
    if instances.is_empty() {
        return Err(resource_error("missing flow build items"));
    }
    Ok(instances)
}
