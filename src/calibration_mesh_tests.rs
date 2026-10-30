use super::*;

fn cube() -> Mesh {
    let p = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    Mesh(
        [
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
        ]
        .map(|f| f.map(|i| p[i]))
        .to_vec(),
    )
}

fn assert_manifold(mesh: &Mesh) {
    let mut edges = BTreeMap::new();
    for f in &mesh.0 {
        let a = std::array::from_fn::<_, 3, _>(|i| f[1][i] - f[0][i]);
        let b = std::array::from_fn::<_, 3, _>(|i| f[2][i] - f[0][i]);
        assert!(
            [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
                .iter()
                .any(|v| *v != 0.0),
            "degenerate {f:?}"
        );
        for i in 0..3 {
            let a = key(f[i]);
            let b = key(f[(i + 1) % 3]);
            let entry = edges.entry(if a < b { (a, b) } else { (b, a) }).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += if a < b { 1 } else { -1 };
        }
    }
    for (edge, counts) in edges {
        assert_eq!(counts, (2, 0), "edge {edge:?}");
    }
}

#[test]
fn cube_cuts_are_capped_and_manifold_including_existing_vertices() {
    for lower in [true, false] {
        for z in [0.5, 1.0] {
            let mut mesh = cube();
            // Tilt the top so the plane also passes through an existing vertex.
            for p in mesh.0.iter_mut().flatten() {
                p[2] += p[0];
            }
            mesh.cut(z, lower).unwrap();
            assert!(!mesh.0.is_empty());
            assert_manifold(&mesh);
            assert!(mesh.0.iter().any(|f| f.iter().all(|p| p[2] == z)));
        }
    }
}

#[test]
fn hollow_cube_keeps_its_hole() {
    let mut mesh = cube();
    let mut inner = cube();
    for f in &mut inner.0 {
        for p in f.iter_mut() {
            p[0] = 0.25 + p[0] * 0.5;
            p[1] = 0.25 + p[1] * 0.5;
        }
        f.swap(1, 2);
    }
    // Join the outside and inside walls with annular top and bottom faces.
    mesh.0.drain(..4);
    inner.0.drain(..4);
    mesh.0.extend(inner.0);
    for z in [0., 1.] {
        let outer = [[0., 0., z], [1., 0., z], [1., 1., z], [0., 1., z]];
        let inner = [[0.25, 0.25, z], [0.75, 0.25, z], [0.75, 0.75, z], [0.25, 0.75, z]];
        for i in 0..4 {
            let j = (i + 1) % 4;
            for mut f in [[outer[i], outer[j], inner[j]], [outer[i], inner[j], inner[i]]] {
                if z == 0. {
                    f.swap(1, 2);
                }
                mesh.0.push(f);
            }
        }
    }
    assert_manifold(&mesh);
    mesh.cut(0.5, true).unwrap();
    assert_manifold(&mesh);
    let cap_area: f64 = mesh
        .0
        .iter()
        .filter(|f| f.iter().all(|p| p[2] == 0.5))
        .map(|f| {
            ((f[1][0] - f[0][0]) * (f[2][1] - f[0][1]) - (f[1][1] - f[0][1]) * (f[2][0] - f[0][0]))
                .abs()
                / 2.
        })
        .sum();
    assert_eq!(cap_area, 0.75);
}

#[test]
fn build_transforms_place_each_flow_instance_and_reject_invalid_matrices() {
    let xml = |transform: &str| {
        format!(
            r#"<model><resources><object id="7" name="flowrate_0.02"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="7" transform="{transform}"/></build></model>"#
        )
    };
    let models = flow_xml(&xml("1 0 0 0 1 0 0 0 1 -93 0 0")).unwrap();
    assert_eq!(models[0].1.bounds().0, [-93., 0., 0.]);
    for transform in ["1 0", "1 0 0 0 1 0 0 0 1 NaN 0 0"] {
        assert!(flow_xml(&xml(transform)).is_err());
    }
}

#[tokio::test]
#[ignore = "requires checksum-pinned Orca CLI and resources"]
async fn reviewed_resource_cuts_are_manifold_after_stl_serialization() {
    let dir = std::env::temp_dir().join(format!("review-cuts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (resource, z, bottom) in [
        ("temperature_tower/temperature_tower.drc", 310.0001, Some(280.0001)),
        ("retraction/retraction_tower.drc", 3.4, None),
        ("volumetric_speed/SpeedTestStructure.drc", 14.1, None),
    ] {
        let mut mesh =
            Mesh::tower(&dir, resource, &std::env::var("ORCA_CLI_PATH").unwrap()).await.unwrap();
        mesh.on_bed();
        mesh.cut(z, true).unwrap();
        if let Some(z) = bottom {
            mesh.cut(z, false).unwrap();
        }
        mesh.on_bed();
        let path = dir.join("cut.stl");
        mesh.write(&path).await.unwrap();
        assert_manifold(&Mesh::stl(&std::fs::read(path).unwrap()).unwrap());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
