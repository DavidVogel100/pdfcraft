//! Measurements on a model, as a viewer's Measure tool takes them: the faces a user picks, their
//! [`Shape`]s found again ([`crate::shapes`]), and what lies between them.
//!
//! Between surfaces whose shapes are known the result is exact to the model's own precision:
//! the distance between parallel planes, a hole's diameter, the distance between parallel axes,
//! the angle between planes. Anything else is measured between the points picked, and says so.

use craft_prc::TriangleMesh;

use crate::shapes::{Fit, Shape, fit_face_in};
use crate::vec3::{add, cross, dot, length, normalize, scale, sub};

/// Directions within this angle count as parallel (or square), radians: half a degree.
const PARALLEL: f64 = 0.5 * std::f64::consts::PI / 180.0;
/// A cylinder that goes this far round (radians) is a hole or a pin: its diameter is read;
/// less, and it is a fillet or a round, read by its radius.
const WHOLE: f64 = 1.9 * std::f64::consts::PI;

/// A point picked on the model, on one of its meshes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    /// The mesh, by index.
    pub mesh: usize,
    /// The CAD face of that mesh the point lies on ([`TriangleMesh::faces`]), when it has faces.
    pub face: Option<usize>,
    /// Where, world coordinates.
    pub point: [f64; 3],
}

/// A picked face with its surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picked {
    pub pick: Pick,
    /// The face's shape, [`Shape::Other`] for a face it isn't known for.
    pub fit: Option<Fit>,
}

/// One reading: a value and where to draw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// A hole's, pin's or ball's diameter: `at` is a point on the surface, `centre` on the axis
    /// (or the ball's centre) level with it.
    Diameter { value: f64, centre: [f64; 3], at: [f64; 3] },
    /// A fillet's or round's radius.
    Radius { value: f64, centre: [f64; 3], at: [f64; 3] },
    /// A straight distance from `from` to `to`; `exact` when it is between surfaces of known
    /// shape, else between the points picked.
    Distance { value: f64, from: [f64; 3], to: [f64; 3], exact: bool },
    /// An angle in degrees (0 to 90 between planes or axes), drawn at `at`.
    Angle { degrees: f64, at: [f64; 3] },
    /// An arc edge's length along its curve, drawn at `at` on it.
    ArcLength { value: f64, at: [f64; 3] },
}

/// The CAD face of `mesh` nearest `point` (the one a click on it hit), by its triangles.
#[must_use]
pub fn face_at(mesh: &TriangleMesh, point: [f64; 3]) -> Option<usize> {
    let corner = |i: u32| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i).copied());
    let nearest = mesh
        .triangles
        .iter()
        .enumerate()
        .filter_map(|(t, tri)| {
            let [a, b, c] = tri.map(corner);
            Some((t, triangle_distance(point, a?, b?, c?)))
        })
        .filter(|(_, d)| d.is_finite())
        .min_by(|x, y| x.1.total_cmp(&y.1))?
        .0;
    mesh.faces.iter().position(|f| f.contains(&nearest))
}

/// `pick` with its face's shape, fitted from the meshes.
#[must_use]
pub fn picked(meshes: &[crate::WorldMesh], pick: Pick) -> Picked {
    let fit = pick.face.and_then(|f| {
        let mesh = &meshes.get(pick.mesh)?.mesh;
        mesh.faces.get(f)?;
        Some(fit_face_in(mesh, f, pick.point))
    });
    Picked { pick, fit }
}

fn shape(p: &Picked) -> Shape {
    p.fit.map_or(Shape::Other, |f| f.shape)
}

/// `p`'s shape as [`between`] measures it: an arc edge as the round it bounds (its centre line
/// as the axis), a straight edge as a round of no size along it.
fn as_measured(p: &Picked) -> Shape {
    match shape(p) {
        Shape::Circle { centre, axis, radius, sweep } => Shape::Cylinder { point: centre, axis, radius, sweep },
        Shape::Line { from, to } => {
            normalize(sub(to, from)).map_or(Shape::Other, |axis| Shape::Cylinder { point: from, axis, radius: 0.0, sweep: 0.0 })
        }
        other => other,
    }
}

/// A picked edge, measured as what it is (a [`Shape::Line`] or [`Shape::Circle`]), picked at
/// `point` on mesh `mesh`.
#[must_use]
pub fn picked_edge(edge: &crate::edges::Edge, point: [f64; 3]) -> Picked {
    let shape = edge.shape();
    let fit = Fit { shape, deviation: 0.0, tolerance: 0.0, corners: edge.path.len() };
    Picked { pick: Pick { mesh: edge.mesh, face: None, point }, fit: Some(fit) }
}

/// What one picked face reads on its own: a hole's diameter, a fillet's radius, a ball's
/// diameter. Nothing for a plane or an unknown face.
#[must_use]
pub fn single(p: &Picked) -> Option<Reading> {
    match shape(p) {
        Shape::Cylinder { point, axis, radius, sweep } => {
            let centre = nearest_on_line(p.pick.point, point, axis);
            let at = on_cylinder(p.pick.point, centre, radius);
            Some(if sweep >= WHOLE { Reading::Diameter { value: 2.0 * radius, centre, at } } else { Reading::Radius { value: radius, centre, at } })
        }
        Shape::Sphere { centre, radius } => {
            let at = normalize(sub(p.pick.point, centre)).map_or(p.pick.point, |d| add(centre, scale(d, radius)));
            Some(Reading::Diameter { value: 2.0 * radius, centre, at })
        }
        // An edge: a straight one's length, an arc's radius or a circle's diameter.
        Shape::Line { from, to } => Some(Reading::Distance { value: length(sub(to, from)), from, to, exact: true }),
        Shape::Circle { centre, axis, radius, sweep } => {
            let d = sub(p.pick.point, centre);
            let at = normalize(sub(d, scale(axis, dot(d, axis)))).map_or(p.pick.point, |u| add(centre, scale(u, radius)));
            Some(if sweep >= WHOLE { Reading::Diameter { value: 2.0 * radius, centre, at } } else { Reading::Radius { value: radius, centre, at } })
        }
        Shape::Plane { .. } | Shape::Other => None,
    }
}

/// An arc edge's length round its curve (nothing for a whole circle or anything else).
#[must_use]
pub fn arc_length(p: &Picked) -> Option<Reading> {
    match shape(p) {
        Shape::Circle { radius, sweep, .. } if sweep < WHOLE => Some(Reading::ArcLength { value: radius * sweep, at: p.pick.point }),
        _ => None,
    }
}

/// What lies between two picked faces: an exact distance or angle where their shapes allow
/// one, else the distance between the points picked.
#[must_use]
pub fn between(a: &Picked, b: &Picked) -> Vec<Reading> {
    let (pa, pb) = (a.pick.point, b.pick.point);
    let loose = Reading::Distance { value: length(sub(pb, pa)), from: pa, to: pb, exact: false };
    let exact = |from: [f64; 3], to: [f64; 3]| Reading::Distance { value: length(sub(to, from)), from, to, exact: true };
    let angle = |u: [f64; 3], v: [f64; 3], at: [f64; 3]| Reading::Angle { degrees: dot(u, v).abs().clamp(0.0, 1.0).acos().to_degrees(), at };
    match (as_measured(a), as_measured(b)) {
        (Shape::Plane { normal: n1, .. }, Shape::Plane { point: q2, normal: n2 }) => {
            if parallel(n1, n2) {
                // From the first pick straight across to the second plane.
                vec![exact(pa, project_to_plane(pa, q2, n2))]
            } else {
                vec![angle(n1, n2, midpoint(pa, pb))]
            }
        }
        (Shape::Cylinder { point: c1, axis: x1, .. }, Shape::Cylinder { point: c2, axis: x2, .. }) => {
            let from = nearest_on_line(pa, c1, x1);
            if parallel(x1, x2) {
                vec![exact(from, nearest_on_line(from, c2, x2))]
            } else {
                let (s, t) = closest_between_lines(c1, x1, c2, x2);
                vec![exact(s, t), angle(x1, x2, midpoint(s, t))]
            }
        }
        (Shape::Cylinder { point, axis, .. }, Shape::Plane { point: q, normal: n }) => {
            axis_and_plane(nearest_on_line(pa, point, axis), axis, q, n, true)
        }
        (Shape::Plane { point: q, normal: n }, Shape::Cylinder { point, axis, .. }) => {
            axis_and_plane(nearest_on_line(pb, point, axis), axis, q, n, false)
        }
        (Shape::Sphere { centre: c1, .. }, Shape::Sphere { centre: c2, .. }) => vec![exact(c1, c2)],
        (Shape::Sphere { centre, .. }, Shape::Plane { point: q, normal: n }) => vec![exact(centre, project_to_plane(centre, q, n))],
        (Shape::Plane { point: q, normal: n }, Shape::Sphere { centre, .. }) => vec![exact(project_to_plane(centre, q, n), centre)],
        (Shape::Sphere { centre, .. }, Shape::Cylinder { point, axis, .. }) => vec![exact(centre, nearest_on_line(centre, point, axis))],
        (Shape::Cylinder { point, axis, .. }, Shape::Sphere { centre, .. }) => vec![exact(nearest_on_line(centre, point, axis), centre)],
        _ => vec![loose],
    }
}

/// An axis (through `on`, along `axis`) and a plane: the distance when the axis runs along the
/// plane, the angle when it crosses it. `axis_first` orders the distance's ends as picked.
fn axis_and_plane(on: [f64; 3], axis: [f64; 3], q: [f64; 3], n: [f64; 3], axis_first: bool) -> Vec<Reading> {
    if square(axis, n) {
        let foot = project_to_plane(on, q, n);
        let (from, to) = if axis_first { (on, foot) } else { (foot, on) };
        vec![Reading::Distance { value: length(sub(to, from)), from, to, exact: true }]
    } else {
        // The angle between the axis and the plane itself.
        vec![Reading::Angle { degrees: 90.0 - dot(axis, n).abs().clamp(0.0, 1.0).acos().to_degrees(), at: on }]
    }
}

fn parallel(u: [f64; 3], v: [f64; 3]) -> bool {
    dot(u, v).abs() >= PARALLEL.cos()
}

fn square(u: [f64; 3], v: [f64; 3]) -> bool {
    dot(u, v).abs() <= PARALLEL.sin()
}

fn midpoint(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    scale(add(a, b), 0.5)
}

fn project_to_plane(p: [f64; 3], q: [f64; 3], n: [f64; 3]) -> [f64; 3] {
    sub(p, scale(n, dot(sub(p, q), n)))
}

/// The point on the line through `on` along unit `axis` nearest `p`.
fn nearest_on_line(p: [f64; 3], on: [f64; 3], axis: [f64; 3]) -> [f64; 3] {
    add(on, scale(axis, dot(sub(p, on), axis)))
}

/// `p` moved straight out from the axis point `centre` onto the cylinder of `radius`.
fn on_cylinder(p: [f64; 3], centre: [f64; 3], radius: f64) -> [f64; 3] {
    normalize(sub(p, centre)).map_or(p, |d| add(centre, scale(d, radius)))
}

/// The nearest points of two lines that are not parallel.
fn closest_between_lines(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let r = sub(p1, p2);
    let (b, c, e) = (dot(d1, d2), dot(d1, r), dot(d2, r));
    let denom = 1.0 - b * b;
    if denom.abs() < 1e-15 {
        return (p1, nearest_on_line(p1, p2, d2));
    }
    let s = (b * e - c) / denom;
    let t = (e - b * c) / denom;
    (add(p1, scale(d1, s)), add(p2, scale(d2, t)))
}

/// The distance from `p` to the triangle `a b c`.
fn triangle_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let n = cross(sub(b, a), sub(c, a));
    let Some(u) = normalize(n) else {
        // A degenerate triangle: its nearest corner.
        return [a, b, c].iter().map(|&q| length(sub(p, q))).fold(f64::INFINITY, f64::min);
    };
    let foot = sub(p, scale(u, dot(sub(p, a), u)));
    // Inside when the foot lies on the inner side of every edge.
    let inside = [(a, b), (b, c), (c, a)].iter().all(|&(s, e)| dot(cross(sub(e, s), sub(foot, s)), n) >= 0.0);
    if inside {
        return dot(sub(p, a), u).abs();
    }
    [(a, b), (b, c), (c, a)].iter().map(|&(s, e)| segment_distance(p, s, e)).fold(f64::INFINITY, f64::min)
}

fn segment_distance(p: [f64; 3], s: [f64; 3], e: [f64; 3]) -> f64 {
    let d = sub(e, s);
    let len2 = dot(d, d);
    let t = if len2 > 0.0 { (dot(sub(p, s), d) / len2).clamp(0.0, 1.0) } else { 0.0 };
    length(sub(p, add(s, scale(d, t))))
}

#[cfg(test)]
mod tests;
