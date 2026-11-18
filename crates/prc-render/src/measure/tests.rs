#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::shapes::Fit;

fn fitted(shape: Shape, point: [f64; 3]) -> Picked {
    Picked { pick: Pick { mesh: 0, face: Some(0), point }, fit: Some(Fit { shape, deviation: 0.0, tolerance: 1e-6, corners: 100 }) }
}

fn plane(z: f64, at: [f64; 3]) -> Picked {
    fitted(Shape::Plane { point: [0.0, 0.0, z], normal: [0.0, 0.0, 1.0] }, at)
}

fn hole(x: f64, radius: f64, sweep: f64, at: [f64; 3]) -> Picked {
    fitted(Shape::Cylinder { point: [x, 0.0, 0.0], axis: [0.0, 0.0, 1.0], radius, sweep }, at)
}

fn distance(r: &[Reading]) -> (f64, bool) {
    match r {
        [Reading::Distance { value, exact, .. }, ..] => (*value, *exact),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_hole_reads_its_diameter_and_a_fillet_its_radius() {
    let Some(Reading::Diameter { value, centre, at }) = single(&hole(2.0, 0.375, std::f64::consts::TAU, [2.375, 0.0, 1.0])) else { panic!() };
    assert!((value - 0.75).abs() < 1e-12);
    assert_eq!(centre, [2.0, 0.0, 1.0]);
    assert!((at[0] - 2.375).abs() < 1e-12);
    assert!(matches!(single(&hole(0.0, 0.125, 1.5, [0.125, 0.0, 0.0])), Some(Reading::Radius { value, .. }) if (value - 0.125).abs() < 1e-12));
    assert_eq!(single(&plane(0.0, [0.0; 3])), None);
}

#[test]
fn parallel_planes_are_their_exact_distance_apart_and_others_meet_at_an_angle() {
    let r = between(&plane(1.0, [3.0, 4.0, 1.0]), &plane(-0.5, [9.0, 9.0, -0.5]));
    assert_eq!(distance(&r), (1.5, true));
    let tilted = fitted(Shape::Plane { point: [0.0; 3], normal: [0.0, 1.0, 0.0] }, [0.0; 3]);
    assert!(matches!(between(&plane(0.0, [0.0; 3]), &tilted)[..], [Reading::Angle { degrees, .. }] if (degrees - 90.0).abs() < 1e-9));
}

#[test]
fn two_holes_are_their_centres_apart() {
    let r = between(&hole(0.0, 0.25, 7.0, [0.25, 0.0, 0.3]), &hole(1.5, 0.25, 7.0, [1.75, 0.0, 0.9]));
    let (value, exact) = distance(&r);
    assert!((value - 1.5).abs() < 1e-12 && exact);
}

#[test]
fn a_hole_and_a_face_along_it_are_the_axis_to_face_distance() {
    // A vertical hole at x = 2 and the face x = 0.
    let wall = fitted(Shape::Plane { point: [0.0; 3], normal: [1.0, 0.0, 0.0] }, [0.0, 1.0, 1.0]);
    let r = between(&hole(2.0, 0.25, 7.0, [2.25, 0.0, 0.5]), &wall);
    assert_eq!(distance(&r), (2.0, true));
    let r = between(&wall, &hole(2.0, 0.25, 7.0, [2.25, 0.0, 0.5]));
    assert_eq!(distance(&r), (2.0, true));
    // Its axis crosses the top face square: an angle of 90° to it.
    assert!(
        matches!(between(&hole(2.0, 0.25, 7.0, [2.25, 0.0, 0.5]), &plane(3.0, [0.0; 3]))[..], [Reading::Angle { degrees, .. }] if (degrees - 90.0).abs() < 1e-9)
    );
}

#[test]
fn faces_of_no_known_shape_are_measured_point_to_point_and_say_so() {
    let free = |p| Picked { pick: Pick { mesh: 0, face: None, point: p }, fit: None };
    assert_eq!(distance(&between(&free([0.0; 3]), &free([3.0, 4.0, 0.0]))), (5.0, false));
}

#[test]
fn edges_read_as_lengths_and_radii_and_measure_like_faces() {
    let line = crate::edges::Edge { mesh: 0, path: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.5, 0.0, 0.0]] };
    let p = picked_edge(&line, [1.0, 0.0, 0.0]);
    assert!(matches!(single(&p), Some(Reading::Distance { value, exact: true, .. }) if (value - 2.5).abs() < 1e-12));
    // A quarter arc of R 0.5 round (0, 0) in the xy plane.
    let path: Vec<[f64; 3]> =
        (0..=6).map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 6.0).map(|a| [0.5 * a.cos(), 0.5 * a.sin(), 0.0]).collect();
    let arc = picked_edge(&crate::edges::Edge { mesh: 0, path }, [0.5, 0.0, 0.0]);
    assert!(matches!(single(&arc), Some(Reading::Radius { value, .. }) if (value - 0.5).abs() < 1e-12));
    // Its length round the quarter: π/2 × 0.5.
    assert!(matches!(arc_length(&arc), Some(Reading::ArcLength { value, .. }) if (value - std::f64::consts::FRAC_PI_4).abs() < 1e-9));
    assert_eq!(arc_length(&p), None, "a straight edge has its length already");
    // The arc's centre to a parallel straight edge 2 away.
    let far = picked_edge(&crate::edges::Edge { mesh: 0, path: vec![[0.0, 2.0, 0.0], [0.0, 2.0, 1.0]] }, [0.0, 2.0, 0.5]);
    let r = between(&arc, &far);
    assert!(matches!(r.first(), Some(Reading::Distance { value, exact: true, .. }) if (value - 2.0).abs() < 1e-12), "{r:?}");
}

#[test]
fn a_click_finds_the_cad_face_under_it() {
    let mut m = TriangleMesh::default();
    m.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [5.0, 0.0, 0.0], [6.0, 0.0, 0.0], [5.0, 1.0, 0.0]];
    m.triangles = vec![[0, 1, 2], [3, 4, 5]];
    m.faces = vec![0..1, 1..2];
    assert_eq!(face_at(&m, [0.2, 0.2, 0.0]), Some(0));
    assert_eq!(face_at(&m, [5.2, 0.3, 0.01]), Some(1));
    assert_eq!(face_at(&TriangleMesh::default(), [0.0; 3]), None);
}
