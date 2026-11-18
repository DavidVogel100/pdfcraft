#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;

/// A face tessellated as a CAD program does it: a grid of `rows` × `cols` corners from `at(u, v)`
/// (u, v in 0..=1), two triangles per cell.
fn grid(rows: u32, cols: u32, at: impl Fn(f64, f64) -> [f64; 3]) -> TriangleMesh {
    let mut m = TriangleMesh::default();
    for i in 0..=rows {
        for j in 0..=cols {
            m.positions.push(at(f64::from(i) / f64::from(rows), f64::from(j) / f64::from(cols)));
        }
    }
    let k = |i: u32, j: u32| i * (cols + 1) + j;
    for i in 0..rows {
        for j in 0..cols {
            m.triangles.push([k(i, j), k(i + 1, j), k(i + 1, j + 1)]);
            m.triangles.push([k(i, j), k(i + 1, j + 1), k(i, j + 1)]);
        }
    }
    let n = m.triangles.len();
    m.faces = std::iter::once(0..n).collect();
    m
}

fn fit(m: &TriangleMesh) -> Fit {
    fit_face(m, 0..m.triangles.len())
}

/// `v` rounded to a grid of `step`, as a compressed tessellation stores its corners.
fn rounded(v: [f64; 3], step: f64) -> [f64; 3] {
    v.map(|c| (c / step).round() * step)
}

#[test]
fn a_flat_face_is_a_plane_through_it() {
    let m = grid(4, 6, |u, v| [10.0 * u, 3.0 + 0.0 * v, 20.0 * v]);
    let f = fit(&m);
    let Shape::Plane { point, normal } = f.shape else { panic!("{f:?}") };
    assert!((normal[1].abs() - 1.0).abs() < 1e-12, "{normal:?}");
    assert!((point[1] - 3.0).abs() < 1e-12);
    assert!(f.deviation < 1e-12);
    assert_eq!(f.corners, 35);
}

#[test]
fn a_hole_is_a_cylinder_of_its_exact_radius_along_its_axis() {
    // A 10 mm hole, 25 mm deep, along a tilted axis, its corners on the exact surface.
    let axis = normalize([1.0, 2.0, 3.0]).unwrap();
    let u = normalize(cross(axis, [1.0, 0.0, 0.0])).unwrap();
    let w = cross(axis, u);
    let origin = [5.0, -7.0, 2.0];
    let at = |s: f64, t: f64| {
        let a = s * std::f64::consts::TAU;
        add(add(origin, scale(axis, 25.0 * t)), add(scale(u, 5.0 * a.cos()), scale(w, 5.0 * a.sin())))
    };
    let f = fit(&grid(48, 6, at));
    let Shape::Cylinder { point, axis: found, radius, sweep } = f.shape else { panic!("{f:?}") };
    assert!((radius - 5.0).abs() < 1e-9, "Ø {}", 2.0 * radius);
    assert!((dot(found, axis).abs() - 1.0).abs() < 1e-12);
    assert!(off_axis(point, origin, axis) < 1e-9, "the axis goes through the hole's centre");
    assert!((sweep - std::f64::consts::TAU).abs() < 0.2, "all the way round: {sweep}");
}

#[test]
fn a_fillet_is_a_quarter_cylinder_and_rounding_keeps_its_radius_close() {
    // A 3 mm fillet a quarter of the way round, corners rounded to 1 µm.
    let at = |s: f64, t: f64| {
        let a = s * std::f64::consts::FRAC_PI_2;
        rounded([3.0 * a.cos(), 3.0 * a.sin(), 40.0 * t], 0.001)
    };
    let f = fit(&grid(12, 10, at));
    let Shape::Cylinder { radius, sweep, .. } = f.shape else { panic!("{f:?}") };
    assert!((radius - 3.0).abs() < 1e-3, "R {radius}");
    assert!((sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-3, "a quarter: {sweep}");
}

#[test]
fn a_fillet_two_corners_wide_is_a_cylinder_not_the_sphere_through_its_ends() {
    let at = |s: f64, t: f64| {
        let a = s * std::f64::consts::FRAC_PI_2;
        [0.375 * a.cos(), 0.375 * a.sin(), 6.0 * t]
    };
    let f = fit(&grid(6, 1, at));
    let Shape::Cylinder { radius, .. } = f.shape else { panic!("{f:?}") };
    assert!((radius - 0.375).abs() < 1e-9);
}

#[test]
fn a_ball_is_a_sphere() {
    let at = |s: f64, t: f64| {
        let (lat, lon) = ((t - 0.5) * 2.8, s * std::f64::consts::TAU);
        [1.0 + 4.0 * lat.cos() * lon.cos(), 2.0 + 4.0 * lat.cos() * lon.sin(), 3.0 + 4.0 * lat.sin()]
    };
    let f = fit(&grid(24, 12, at));
    let Shape::Sphere { centre, radius } = f.shape else { panic!("{f:?}") };
    assert!((radius - 4.0).abs() < 1e-9);
    assert!(length(sub(centre, [1.0, 2.0, 3.0])) < 1e-9);
}

#[test]
fn a_free_form_face_is_none_of_them() {
    let f = fit(&grid(10, 10, |u, v| [10.0 * u, 10.0 * v, (u * 6.0).sin() * (v * 4.0).cos()]));
    assert_eq!(f.shape, Shape::Other);
}

#[test]
fn a_face_with_too_few_or_broken_corners_is_other_and_never_panics() {
    let mut m = TriangleMesh::default();
    assert_eq!(fit_face(&m, 0..0).shape, Shape::Other);
    m.positions = vec![[0.0; 3], [1.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0]];
    m.triangles = vec![[0, 1, 2], [0, 1, 9]];
    assert_eq!(fit_face(&m, 0..2).shape, Shape::Other);
    assert_eq!(fit_face(&m, 5..9).shape, Shape::Other);
}

#[test]
fn eigenvectors_and_solves_hold_for_simple_cases() {
    let v = smallest_eigenvector([[3.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 2.0]]).unwrap();
    assert!((v[1].abs() - 1.0).abs() < 1e-12);
    assert_eq!(solve3([[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 4.0]], [1.0, 1.0, 1.0]), Some([1.0, 0.5, 0.25]));
    assert_eq!(solve3([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]], [1.0, 1.0, 1.0]), None);
}

#[test]
fn a_fillet_one_strip_wide_takes_its_radius_from_the_faces_it_joins() {
    // An L of two plates meeting round a 0.25 fillet: a floor (z = 0, x from 0.25 to 2), a wall
    // (x = 0, z from 0.25 to 2), and the fillet between them, centred on (0.25, *, 0.25), its
    // corners on the arc but only one strip of triangles across.
    let (r, len) = (0.25, 3.0);
    let mut m = TriangleMesh::default();
    let quad = |m: &mut TriangleMesh, corners: [[f64; 3]; 4]| {
        let n = u32::try_from(m.positions.len()).unwrap();
        m.positions.extend(corners);
        let start = m.triangles.len();
        m.triangles.extend([[n, n + 1, n + 2], [n, n + 2, n + 3]]);
        m.faces.push(start..start + 2);
    };
    quad(&mut m, [[r, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, len, 0.0], [r, len, 0.0]]);
    quad(&mut m, [[0.0, 0.0, r], [0.0, len, r], [0.0, len, 2.0], [0.0, 0.0, 2.0]]);
    // The fillet strip from the floor's edge (x = r, z = 0) to the wall's (x = 0, z = r).
    quad(&mut m, [[r, 0.0, 0.0], [r, len, 0.0], [0.0, len, r], [0.0, 0.0, r]]);
    assert!(matches!(fit_face(&m, m.faces[2].clone()).shape, Shape::Plane { .. }), "on its own it lies flat");
    let f = fit_face_in(&m, 2);
    let Shape::Cylinder { radius, sweep, axis, point } = f.shape else { panic!("{f:?}") };
    assert!((radius - r).abs() < 1e-12, "R {radius}");
    assert!((sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert!((axis[1].abs() - 1.0).abs() < 1e-12);
    assert!((point[0] - r).abs() < 1e-12 && (point[2] - r).abs() < 1e-12, "{point:?}");
    // The plates themselves stay planes.
    assert!(matches!(fit_face_in(&m, 0).shape, Shape::Plane { .. }));
    // A rounded outside edge the same way: the strip from the top's edge (x = 1.75, z = 2) to the
    // side's (x = 2, z = 1.75), round a centre at (1.75, *, 1.75).
    let mut m = TriangleMesh::default();
    quad(&mut m, [[0.0, 0.0, 2.0], [1.75, 0.0, 2.0], [1.75, len, 2.0], [0.0, len, 2.0]]);
    quad(&mut m, [[2.0, 0.0, 0.0], [2.0, len, 0.0], [2.0, len, 1.75], [2.0, 0.0, 1.75]]);
    quad(&mut m, [[1.75, 0.0, 2.0], [1.75, len, 2.0], [2.0, len, 1.75], [2.0, 0.0, 1.75]]);
    let Shape::Cylinder { radius, point, .. } = fit_face_in(&m, 2).shape else { panic!() };
    assert!((radius - r).abs() < 1e-12 && (point[0] - 1.75).abs() < 1e-12 && (point[2] - 1.75).abs() < 1e-12, "{radius} {point:?}");
}
