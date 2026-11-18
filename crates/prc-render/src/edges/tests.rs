#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;

/// A mesh whose outline is `points` joined in order (closed when `closed`), as segments.
fn chain(points: &[[f64; 3]], closed: bool) -> (TriangleMesh, Vec<[u32; 2]>) {
    let mut m = TriangleMesh::default();
    m.positions = points.to_vec();
    let n = u32::try_from(points.len()).unwrap();
    let mut segs: Vec<[u32; 2]> = (0..n - 1).map(|i| [i, i + 1]).collect();
    if closed {
        segs.push([n - 1, 0]);
    }
    (m, segs)
}

fn arc(r: f64, from: f64, to: f64, n: u32) -> Vec<[f64; 3]> {
    (0..=n).map(|i| from + (to - from) * f64::from(i) / f64::from(n)).map(|a| [1.0 + r * a.cos(), 2.0 + r * a.sin(), 3.0]).collect()
}

#[test]
fn an_arc_is_followed_from_any_of_its_segments_and_reads_its_radius() {
    let points = arc(0.25, 0.0, std::f64::consts::FRAC_PI_2, 8);
    let (m, segs) = chain(&points, false);
    let edge = Edge { mesh: 0, path: follow(&m, &segs, 3) };
    assert_eq!(edge.path.len(), 9, "the whole arc, both ways from the segment picked");
    let Shape::Circle { radius, sweep, centre, .. } = edge.shape() else { panic!("{:?}", edge.shape()) };
    assert!((radius - 0.25).abs() < 1e-12);
    assert!((sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    assert!(length(sub(centre, [1.0, 2.0, 3.0])) < 1e-12);
}

#[test]
fn a_straight_edge_stops_at_a_corner_and_reads_its_length() {
    // An L: 3 along x in three pieces, then 2 up y.
    let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 2.0, 0.0]];
    let (m, segs) = chain(&points, false);
    let edge = Edge { mesh: 0, path: follow(&m, &segs, 1) };
    assert_eq!(edge.shape(), Shape::Line { from: [0.0, 0.0, 0.0], to: [3.0, 0.0, 0.0] });
    let other = Edge { mesh: 0, path: follow(&m, &segs, 3) };
    assert_eq!(other.shape(), Shape::Line { from: [3.0, 0.0, 0.0], to: [3.0, 2.0, 0.0] });
}

#[test]
fn a_hole_edge_goes_all_the_way_round_and_reads_as_a_whole_circle() {
    let mut points = arc(0.1875, 0.0, std::f64::consts::TAU, 24);
    points.pop();
    let (m, segs) = chain(&points, true);
    let edge = Edge { mesh: 0, path: follow(&m, &segs, 5) };
    let Shape::Circle { radius, sweep, .. } = edge.shape() else { panic!("{:?}", edge.shape()) };
    assert!((radius - 0.1875).abs() < 1e-12);
    assert!(sweep > 6.2, "round: {sweep}");
}

#[test]
fn an_arc_running_smoothly_into_a_straight_is_read_as_the_piece_picked() {
    // A quarter arc of R 0.5 round (0, 0) from (0.5, 0) to (0, 0.5), then straight on to (-2, 0.5).
    let mut points = arc(0.5, 0.0, std::f64::consts::FRAC_PI_2, 6).into_iter().map(|p| [p[0] - 1.0, p[1] - 2.0, 0.0]).collect::<Vec<_>>();
    points.extend([[-1.0, 0.5, 0.0], [-2.0, 0.5, 0.0]]);
    let (m, segs) = chain(&points, false);
    let whole = Edge { mesh: 0, path: follow(&m, &segs, 2) };
    assert_eq!(whole.path.len(), 9, "followed round the smooth join");
    assert_eq!(whole.shape(), Shape::Other);
    // Picked on the curve: the arc alone.
    let on_arc = whole.piece([0.5 * 0.7071, 0.5 * 0.7071, 0.0]);
    let Shape::Circle { radius, sweep, .. } = on_arc.shape() else { panic!("{:?}", on_arc.shape()) };
    assert!((radius - 0.5).abs() < 1e-12 && (sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    // Picked on the straight: the straight alone, from where the arc ends.
    assert_eq!(whole.piece([-1.5, 0.5, 0.0]).shape(), Shape::Line { from: [0.0, 0.5, 0.0], to: [-2.0, 0.5, 0.0] });
}

#[test]
fn a_joint_where_three_edges_meet_ends_the_edge() {
    // A T: a straight edge along x, with a branch up y from its middle.
    let mut m = TriangleMesh::default();
    m.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
    let segs = vec![[0, 1], [1, 2], [1, 3]];
    assert_eq!(follow(&m, &segs, 0), vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]);
}
