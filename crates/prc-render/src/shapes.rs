//! The CAD surfaces behind a tessellated face, found again from its triangles.
//!
//! A CAD program tessellates each face of a solid by placing the triangles' corners on the
//! exact surface; only the flat triangles between them cut across a curve. So a face's corners
//! pin down the surface they were sampled from: a plane, a cylinder (a hole, a pin, a tube, a
//! fillet), or a sphere, measured as exactly as the corners were written. A face that is none of
//! these, within a tolerance, is [`Shape::Other`] and is measured on its triangles.

// The small matrix routines read as the textbook writes them, by row and column.
#![allow(clippy::needless_range_loop)]

use std::ops::Range;

use craft_prc::TriangleMesh;

use crate::vec3::{add, cross, dot, length, normalize, scale, sub};

/// A face's surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// The points `p` with `normal · (p - point) = 0`.
    Plane {
        /// A point on it: the face's corners' centroid.
        point: [f64; 3],
        /// Unit normal (its sign is arbitrary).
        normal: [f64; 3],
    },
    /// The points at `radius` from the line through `point` along `axis`.
    Cylinder {
        /// A point on the axis: the face's corners' centroid, moved onto the axis.
        point: [f64; 3],
        /// Unit axis direction (its sign is arbitrary).
        axis: [f64; 3],
        radius: f64,
        /// How much of the full circle the face goes round, radians: 2π for a whole hole or
        /// pin, less for a fillet or a half-round.
        sweep: f64,
    },
    /// The points at `radius` from `centre`.
    Sphere { centre: [f64; 3], radius: f64 },
    /// None of these within [`Fit::tolerance`].
    Other,
}

/// A face's [`Shape`], with how closely its corners lie on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub shape: Shape,
    /// The largest distance of a corner from the shape, model units (0 for [`Shape::Other`]).
    pub deviation: f64,
    /// The deviation allowed for this face: its size times [`RELATIVE_TOLERANCE`], at least
    /// [`ABSOLUTE_TOLERANCE`].
    pub tolerance: f64,
    /// The face's corners.
    pub corners: usize,
}

/// How far a corner may lie off a fitted shape, as a fraction of the face's size: the corners
/// of compressed tessellations are rounded to a grid, a few parts in a hundred thousand.
pub const RELATIVE_TOLERANCE: f64 = 2e-4;
/// The least deviation allowed, model units.
pub const ABSOLUTE_TOLERANCE: f64 = 1e-6;
/// The fewest corners a face is fitted to a cylinder on (it has five numbers to fit).
pub const MIN_CYLINDER_CORNERS: usize = 8;
/// The fewest corners a face is fitted to a sphere on (it has four numbers to fit).
pub const MIN_SPHERE_CORNERS: usize = 10;
/// Faces with more corners than this are fitted on an even sample of them.
const MAX_CORNERS: usize = 20_000;

/// Debugging: each candidate's deviation and the tolerance (not for release).
#[doc(hidden)]
#[must_use]
pub fn candidates(mesh: &TriangleMesh, triangles: Range<usize>) -> Vec<(Shape, f64)> {
    let (points, normals) = corners_of(mesh, triangles);
    [plane(&points), cylinder(&points, &normals), sphere(&points)]
        .into_iter()
        .flatten()
        .map(|s| (s, points.iter().map(|&p| distance(&s, p)).fold(0.0, f64::max)))
        .collect()
}

fn corners_of(mesh: &TriangleMesh, triangles: Range<usize>) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
    let mut seen = std::collections::HashSet::new();
    let (mut points, mut normals) = (Vec::new(), Vec::new());
    for tri in mesh.triangles.get(triangles).unwrap_or(&[]) {
        let corners = tri.map(|i| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i).copied()));
        let [Some(a), Some(b), Some(c)] = corners else { continue };
        if ![a, b, c].iter().flatten().all(|v| v.is_finite()) {
            continue;
        }
        normals.push(cross(sub(b, a), sub(c, a)));
        for (&i, p) in tri.iter().zip([a, b, c]) {
            if seen.insert(i) {
                points.push(p);
            }
        }
    }
    (points, normals)
}

/// The surface of `mesh`'s triangles in `triangles` (one of its [`TriangleMesh::faces`]).
#[must_use]
pub fn fit_face(mesh: &TriangleMesh, triangles: Range<usize>) -> Fit {
    let (mut points, normals) = corners_of(mesh, triangles);
    let corners = points.len();
    if points.len() > MAX_CORNERS {
        let step = points.len().div_ceil(MAX_CORNERS);
        points = points.into_iter().step_by(step).collect();
    }
    let other = |tolerance| Fit { shape: Shape::Other, deviation: 0.0, tolerance, corners };
    let Some((lo, hi)) = bounds(&points) else { return other(ABSOLUTE_TOLERANCE) };
    let size = length(sub(hi, lo));
    let tolerance = (size * RELATIVE_TOLERANCE).max(ABSOLUTE_TOLERANCE);
    if points.len() < 3 {
        return other(tolerance);
    }
    let fit = |shape: Shape| {
        let deviation = points.iter().map(|&p| distance(&shape, p)).fold(0.0, f64::max);
        Fit { shape, deviation, tolerance, corners }
    };
    if let Some(plane) = plane(&points).map(fit).filter(|f| f.deviation <= tolerance) {
        return plane;
    }
    // More corners than the shape has numbers to fit, with some to spare: four corners lie on
    // some sphere whatever the face is.
    let cylinder = (points.len() >= MIN_CYLINDER_CORNERS).then(|| cylinder(&points, &normals)).flatten();
    let sphere = (points.len() >= MIN_SPHERE_CORNERS).then(|| sphere(&points)).flatten();
    // In order of preference: a strip of a cylinder only two corners wide (a fillet) also lies
    // on some sphere, so a face that is a cylinder is taken for one.
    [cylinder, sphere].into_iter().flatten().map(fit).find(|f| f.deviation <= tolerance).unwrap_or(other(tolerance))
}

/// The surface of `mesh`'s face number `face`: as [`fit_face`] finds it, or, for a fillet
/// tessellated too coarsely to show its curve (one strip of triangles across, which lies flat),
/// the round its two neighbouring faces imply: it meets both tangentially, so their angle and
/// its width fix its radius.
#[must_use]
///
/// A face that is none of these as a whole (a producer may make one face of flat sides and the
/// rounded corners between them) is fitted round `near`, the point picked on it: the largest
/// patch of its corners about that point that a cylinder fits.
pub fn fit_face_in(mesh: &TriangleMesh, face: usize, near: [f64; 3]) -> Fit {
    let Some(range) = mesh.faces.get(face).cloned() else {
        return Fit { shape: Shape::Other, deviation: 0.0, tolerance: ABSOLUTE_TOLERANCE, corners: 0 };
    };
    let fit = fit_face(mesh, range.clone());
    match fit.shape {
        Shape::Cylinder { .. } | Shape::Sphere { .. } => fit,
        Shape::Plane { .. } => fillet_between_neighbours(mesh, face, fit.tolerance).map_or(fit, |(shape, deviation)| Fit { shape, deviation, ..fit }),
        Shape::Other => cylinder_near(mesh, range, near).unwrap_or(fit),
    }
}

/// The largest patch of the face's corners nearest `near` that a cylinder fits within
/// tolerance: the curve where the face was picked.
fn cylinder_near(mesh: &TriangleMesh, triangles: Range<usize>, near: [f64; 3]) -> Option<Fit> {
    let tris = mesh.triangles.get(triangles)?;
    let corner = |i: u32| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i).copied());
    // Triangles by their distance from the pick, nearest first.
    let mut by_distance: Vec<(f64, [[f64; 3]; 3])> = tris
        .iter()
        .filter_map(|t| {
            let [a, b, c] = t.map(corner);
            let (a, b, c) = (a?, b?, c?);
            let mid = scale(add(add(a, b), c), 1.0 / 3.0);
            Some((length(sub(mid, near)), [a, b, c]))
        })
        .filter(|(d, _)| d.is_finite())
        .collect();
    by_distance.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut best = None;
    let mut n = 8;
    while n <= by_distance.len().min(MAX_CORNERS) {
        let patch = by_distance.get(..n)?;
        let normals: Vec<[f64; 3]> = patch.iter().map(|(_, [a, b, c])| cross(sub(*b, *a), sub(*c, *a))).collect();
        let mut points: Vec<[f64; 3]> = patch.iter().flat_map(|(_, t)| *t).collect();
        points.sort_by(|p, q| p.map(f64::to_bits).cmp(&q.map(f64::to_bits)));
        points.dedup();
        let (lo, hi) = bounds(&points)?;
        let tolerance = (length(sub(hi, lo)) * RELATIVE_TOLERANCE).max(ABSOLUTE_TOLERANCE);
        let fitted = (points.len() >= MIN_CYLINDER_CORNERS).then(|| cylinder(&points, &normals)).flatten().map(|shape| {
            let deviation = points.iter().map(|&p| distance(&shape, p)).fold(0.0, f64::max);
            Fit { shape, deviation, tolerance, corners: points.len() }
        });
        match fitted {
            // A real curve: it turns at least a few degrees across the patch.
            Some(f) if f.deviation <= tolerance && matches!(f.shape, Shape::Cylinder { sweep, .. } if sweep > 5f64.to_radians()) => best = Some(f),
            // Past where the face stops being one round: keep the last patch that was.
            _ if best.is_some() => break,
            _ => {}
        }
        n = n.saturating_mul(2);
    }
    best
}

/// The fillet face `face` of `mesh` is, from its neighbours: the two faces it shares the most
/// corners with, both planes at an angle; the cylinder tangent to both through the corners it
/// shares with each. With how far its own corners lie from it, which must be within `tolerance`.
fn fillet_between_neighbours(mesh: &TriangleMesh, face: usize, tolerance: f64) -> Option<(Shape, f64)> {
    let key = |p: [f64; 3]| p.map(f64::to_bits);
    let (own, _) = corners_of(mesh, mesh.faces.get(face)?.clone());
    let mine: std::collections::HashSet<[u64; 3]> = own.iter().map(|&p| key(p)).collect();
    // Each other face's corners shared with this one.
    let mut shared: Vec<(usize, Vec<[f64; 3]>)> = mesh
        .faces
        .iter()
        .enumerate()
        .filter(|&(g, _)| g != face)
        .map(|(g, range)| (g, corners_of(mesh, range.clone()).0.into_iter().filter(|&p| mine.contains(&key(p))).collect::<Vec<_>>()))
        .filter(|(_, pts)| pts.len() >= 2)
        .collect();
    shared.sort_by_key(|(_, pts)| std::cmp::Reverse(pts.len()));
    let [(ga, edge_a), (gb, edge_b), ..] = shared.as_slice() else { return None };
    // Each neighbour's surface direction where the fillet meets it: a plane's normal, or a
    // round's (a hole's, a boss's) straight out from its axis or centre there.
    let normal_at = |g: usize, at: [f64; 3]| match fit_face(mesh, mesh.faces.get(g)?.clone()).shape {
        Shape::Plane { normal, .. } => Some(normal),
        Shape::Cylinder { point, axis, .. } => {
            let d = sub(at, point);
            normalize(sub(d, scale(axis, dot(d, axis))))
        }
        Shape::Sphere { centre, .. } => normalize(sub(at, centre)),
        Shape::Other => None,
    };
    let (mut na, mut nb) = (normal_at(*ga, centroid(edge_a))?, normal_at(*gb, centroid(edge_b))?);
    // Both normals turned to the fillet's own side, so the angle between them is its sweep.
    let Shape::Plane { normal: chord_normal, .. } = plane(&own)? else { return None };
    if dot(na, chord_normal) < 0.0 {
        na = scale(na, -1.0);
    }
    if dot(nb, chord_normal) < 0.0 {
        nb = scale(nb, -1.0);
    }
    let sweep = dot(na, nb).clamp(-1.0, 1.0).acos();
    if !(sweep > 2f64.to_radians() && sweep < 178f64.to_radians()) {
        return None;
    }
    let axis = normalize(cross(na, nb))?;
    let (ca, cb) = (centroid(edge_a), centroid(edge_b));
    let across = |v: [f64; 3]| sub(v, scale(axis, dot(v, axis)));
    let chord = length(across(sub(cb, ca)));
    let radius = chord / (2.0 * (sweep / 2.0).sin());
    if !(radius.is_finite() && radius > 0.0) {
        return None;
    }
    // The axis lies a radius from each tangent edge along its face's normal: one way for a
    // fillet in a corner, the other for a rounded edge. The side where both agree is the one.
    let centre = |side: f64| {
        let (from_a, from_b) = (add(ca, scale(na, side * radius)), add(cb, scale(nb, side * radius)));
        (scale(add(from_a, from_b), 0.5), length(across(sub(from_b, from_a))))
    };
    let (point, gap) = [centre(1.0), centre(-1.0)].into_iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    if gap > radius * 0.05 + tolerance {
        return None;
    }
    let shape = Shape::Cylinder { point, axis, radius, sweep };
    let deviation = own.iter().map(|&p| distance(&shape, p)).fold(0.0, f64::max);
    (deviation <= tolerance.max(radius * 1e-3)).then_some((shape, deviation))
}

/// How far `p` lies from `shape` (0 for [`Shape::Other`]).
#[must_use]
pub fn distance(shape: &Shape, p: [f64; 3]) -> f64 {
    match *shape {
        Shape::Plane { point, normal } => dot(sub(p, point), normal).abs(),
        Shape::Cylinder { point, axis, radius, .. } => (off_axis(p, point, axis) - radius).abs(),
        Shape::Sphere { centre, radius } => (length(sub(p, centre)) - radius).abs(),
        Shape::Other => 0.0,
    }
}

/// `p`'s distance from the line through `point` along the unit `axis`.
fn off_axis(p: [f64; 3], point: [f64; 3], axis: [f64; 3]) -> f64 {
    let d = sub(p, point);
    length(sub(d, scale(axis, dot(d, axis))))
}

fn bounds(points: &[[f64; 3]]) -> Option<([f64; 3], [f64; 3])> {
    let first = *points.first()?;
    Some(points.iter().fold((first, first), |(lo, hi), p| ([0, 1, 2].map(|k| lo[k].min(p[k])), [0, 1, 2].map(|k| hi[k].max(p[k])))))
}

fn centroid(points: &[[f64; 3]]) -> [f64; 3] {
    let n = points.len().max(1) as f64;
    scale(points.iter().fold([0.0; 3], |s, &p| add(s, p)), 1.0 / n)
}

/// The scatter matrix of `vectors` (each taken from `about`).
fn scatter(vectors: impl Iterator<Item = [f64; 3]>) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for v in vectors {
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += v[i] * v[j];
            }
        }
    }
    m
}

/// The unit eigenvector of the symmetric `m` with the smallest eigenvalue (Jacobi rotations).
fn smallest_eigenvector(mut m: [[f64; 3]; 3]) -> Option<[f64; 3]> {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let (mut p, mut q, mut big) = (0, 1, 0.0);
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if m[i][j].abs() > big {
                (p, q, big) = (i, j, m[i][j].abs());
            }
        }
        let diagonal = m[0][0].abs() + m[1][1].abs() + m[2][2].abs();
        if big <= 1e-15 * diagonal.max(f64::MIN_POSITIVE) {
            break;
        }
        let theta = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
        // m ← Jᵀ m J and v ← v J, with J the rotation in the (p, q) plane.
        for k in 0..3 {
            let (mkp, mkq) = (m[k][p], m[k][q]);
            m[k][p] = c * mkp - s * mkq;
            m[k][q] = s * mkp + c * mkq;
        }
        for k in 0..3 {
            let (mpk, mqk) = (m[p][k], m[q][k]);
            m[p][k] = c * mpk - s * mqk;
            m[q][k] = s * mpk + c * mqk;
        }
        for row in &mut v {
            let (vp, vq) = (row[p], row[q]);
            row[p] = c * vp - s * vq;
            row[q] = s * vp + c * vq;
        }
    }
    let k = (0..3).min_by(|&a, &b| m[a][a].total_cmp(&m[b][b]))?;
    normalize([v[0][k], v[1][k], v[2][k]])
}

/// The plane through `points`' centroid across their least spread.
fn plane(points: &[[f64; 3]]) -> Option<Shape> {
    let point = centroid(points);
    let normal = smallest_eigenvector(scatter(points.iter().map(|&p| sub(p, point))))?;
    Some(Shape::Plane { point, normal })
}

/// The cylinder whose axis lies across every face normal (`normals`, area-weighted) and whose
/// circle best fits `points` seen along it.
fn cylinder(points: &[[f64; 3]], normals: &[[f64; 3]]) -> Option<Shape> {
    // A cylinder's normals all lie square to its axis: the axis is their least spread.
    let axis = smallest_eigenvector(scatter(normals.iter().filter_map(|&n| normalize(n).map(|u| scale(u, length(n).sqrt())))))?;
    let centre = centroid(points);
    // A frame across the axis.
    let u = normalize(cross(axis, if axis[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }))?;
    let w = cross(axis, u);
    let flat: Vec<[f64; 2]> = points.iter().map(|&p| sub(p, centre)).map(|d| [dot(d, u), dot(d, w)]).collect();
    let (cx, cy, radius) = circle(&flat)?;
    let point = add(centre, add(scale(u, cx), scale(w, cy)));
    // How far round the circle the points go: 2π less the widest empty gap between them.
    let mut angles: Vec<f64> = flat.iter().map(|&[x, y]| (y - cy).atan2(x - cx)).collect();
    angles.sort_by(f64::total_cmp);
    let gap = angles
        .windows(2)
        .map(|w| w[1] - w[0])
        .chain(angles.first().zip(angles.last()).map(|(a, b)| a + std::f64::consts::TAU - b))
        .fold(0.0, f64::max);
    let sweep = std::f64::consts::TAU - gap;
    Some(Shape::Cylinder { point, axis, radius, sweep })
}

/// The circle best fitting `points`: an algebraic (Kåsa) fit, then Gauss–Newton steps on the
/// distances themselves. Centre and radius; `None` for points along a line.
fn circle(points: &[[f64; 2]]) -> Option<(f64, f64, f64)> {
    // x² + y² = 2a·x + 2b·y + c, by least squares.
    let mut ata = [[0.0; 3]; 3];
    let mut atb = [0.0; 3];
    for &[x, y] in points {
        let row = [2.0 * x, 2.0 * y, 1.0];
        let rhs = x * x + y * y;
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * rhs;
        }
    }
    let [a, b, c] = solve3(ata, atb)?;
    let mut centre = [a, b];
    let mut radius = (c + a * a + b * b).sqrt();
    for _ in 0..10 {
        // Residuals r_i = |p_i - centre| - radius; unknowns (centre, radius).
        let mut jtj = [[0.0; 3]; 3];
        let mut jtr = [0.0; 3];
        for &[x, y] in points {
            let (dx, dy) = (x - centre[0], y - centre[1]);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= 0.0 {
                continue;
            }
            let row = [-dx / d, -dy / d, -1.0];
            let r = d - radius;
            for i in 0..3 {
                for j in 0..3 {
                    jtj[i][j] += row[i] * row[j];
                }
                jtr[i] += row[i] * r;
            }
        }
        let Some([sx, sy, sr]) = solve3(jtj, jtr.map(|v| -v)) else { break };
        centre = [centre[0] + sx, centre[1] + sy];
        radius += sr;
        if sx.abs() + sy.abs() + sr.abs() <= 1e-14 * radius.abs().max(1.0) {
            break;
        }
    }
    (radius.is_finite() && radius > 0.0).then_some((centre[0], centre[1], radius))
}

/// The sphere best fitting `points` (algebraic fit).
fn sphere(points: &[[f64; 3]]) -> Option<Shape> {
    // x² + y² + z² = 2a·x + 2b·y + 2c·z + d, by least squares, about the centroid for conditioning.
    let o = centroid(points);
    let mut ata = [[0.0; 4]; 4];
    let mut atb = [0.0; 4];
    for &p in points {
        let [x, y, z] = sub(p, o);
        let row = [2.0 * x, 2.0 * y, 2.0 * z, 1.0];
        let rhs = x * x + y * y + z * z;
        for i in 0..4 {
            for j in 0..4 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * rhs;
        }
    }
    let [a, b, c, d] = solve4(ata, atb)?;
    let radius = (d + a * a + b * b + c * c).sqrt();
    (radius.is_finite() && radius > 0.0).then_some(Shape::Sphere { centre: add(o, [a, b, c]), radius })
}

/// `m x = b` by Gaussian elimination with partial pivoting; `None` when singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let mut a = [[0.0; 4]; 3];
    for i in 0..3 {
        a[i] = [m[i][0], m[i][1], m[i][2], b[i]];
    }
    let x = eliminate(&mut a)?;
    Some([x[0], x[1], x[2]])
}

fn solve4(m: [[f64; 4]; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let mut a = [[0.0; 5]; 4];
    for i in 0..4 {
        a[i] = [m[i][0], m[i][1], m[i][2], m[i][3], b[i]];
    }
    let x = eliminate(&mut a)?;
    Some([x[0], x[1], x[2], x[3]])
}

/// Solves the augmented `N × (N+1)` system in place.
fn eliminate<const N: usize, const M: usize>(a: &mut [[f64; M]; N]) -> Option<[f64; N]> {
    let scale_of = a.iter().flat_map(|r| r.iter().take(N)).fold(0.0f64, |s, v| s.max(v.abs()));
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() <= 1e-14 * scale_of {
            return None;
        }
        a.swap(col, pivot);
        for row in col + 1..N {
            let f = a[row][col] / a[col][col];
            for k in col..M {
                a[row][k] -= f * a[col][k];
            }
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let mut s = a[row][N];
        for k in row + 1..N {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests;
