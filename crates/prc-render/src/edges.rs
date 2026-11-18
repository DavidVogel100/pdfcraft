//! Edges picked to measure, as a CAD program picks them: the drawn edge line nearest a click
//! ([`crate::outline_edges`]), followed along its run of segments until it meets another edge
//! or turns a corner, and read as what it is: a straight edge's length, an arc's radius.

use std::collections::HashMap;

use craft_prc::TriangleMesh;

use crate::shapes::Shape;
use crate::vec3::{add, cross, dot, length, normalize, scale, sub};
use crate::{Camera, Frame};

/// Segments meeting at more than this angle end an edge: a corner, not the next facet of an arc.
const CORNER: f64 = 35.0 * std::f64::consts::PI / 180.0;
/// An edge is straight, or an arc, when its points lie this close to the line or circle, as a
/// share of its length (at least [`ABSOLUTE`]).
const RELATIVE: f64 = 2e-4;
const ABSOLUTE: f64 = 1e-6;
/// The most segments followed along one edge.
const MAX_SEGMENTS: usize = 100_000;

/// An edge picked: its mesh, and the points along it in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub mesh: usize,
    pub path: Vec<[f64; 3]>,
}

impl Edge {
    /// The piece of the edge about `near` that is one straight line or one arc: where an arc
    /// runs smoothly on into a straight (as a fillet meets a wall), CAD programs make two edges,
    /// but a tessellation keeps no mark of where one ends. Grown from the segment nearest `near`
    /// both ways while it stays one or the other.
    #[must_use]
    pub fn piece(&self, near: [f64; 3]) -> Edge {
        let n = self.path.len();
        if n < 3 || !matches!(self.shape(), Shape::Other) {
            return self.clone();
        }
        let gap = |s: usize| {
            let (Some(&a), Some(&b)) = (self.path.get(s), self.path.get(s + 1)) else { return f64::INFINITY };
            let d = sub(b, a);
            let len2 = dot(d, d);
            let t = if len2 > 0.0 { (dot(sub(near, a), d) / len2).clamp(0.0, 1.0) } else { 0.0 };
            length(sub(near, add(a, scale(d, t))))
        };
        let start = (0..n - 1).min_by(|&a, &b| gap(a).total_cmp(&gap(b))).unwrap_or(0);
        let fits = |lo: usize, hi: usize| {
            let Some(points) = self.path.get(lo..=hi) else { return false };
            let piece = Edge { mesh: self.mesh, path: points.to_vec() };
            // Three points always lie on some circle: an arc needs four to show it is one.
            match piece.shape() {
                Shape::Line { .. } => true,
                Shape::Circle { .. } => points.len() >= 4,
                _ => false,
            }
        };
        let (mut lo, mut hi) = (start, start + 1);
        // One point more on either side, else one on each or two on one side (an arc shows only
        // from four points).
        let steps: [(usize, usize); 5] = [(0, 1), (1, 0), (1, 1), (0, 2), (2, 0)];
        while let Some((down, up)) = steps.iter().copied().find(|&(down, up)| lo >= down && hi + up < n && fits(lo - down, hi + up)) {
            (lo, hi) = (lo - down, hi + up);
        }
        Edge { mesh: self.mesh, path: self.path.get(lo..=hi).map_or_else(|| self.path.clone(), <[_]>::to_vec) }
    }

    /// What the edge is: a [`Shape::Line`] for a straight one, a [`Shape::Circle`] for an arc,
    /// else [`Shape::Other`].
    #[must_use]
    pub fn shape(&self) -> Shape {
        let (Some(&first), Some(&last)) = (self.path.first(), self.path.last()) else { return Shape::Other };
        let span: f64 = self.path.windows(2).map(|w| length(sub(w[1], w[0]))).sum();
        let tolerance = (span * RELATIVE).max(ABSOLUTE);
        if let Some(direction) = normalize(sub(last, first)) {
            let off = |p: [f64; 3]| {
                let d = sub(p, first);
                length(sub(d, scale(direction, dot(d, direction))))
            };
            if self.path.iter().all(|&p| off(p) <= tolerance) {
                return Shape::Line { from: first, to: last };
            }
        }
        circle(&self.path, tolerance).unwrap_or(Shape::Other)
    }
}

/// The circle through `path`'s points within `tolerance`, in the plane they lie in; how far
/// round it they go.
fn circle(path: &[[f64; 3]], tolerance: f64) -> Option<Shape> {
    if path.len() < 3 {
        return None;
    }
    let Shape::Plane { point, normal } = crate::shapes::fit_plane(path)? else { return None };
    if path.iter().any(|&p| dot(sub(p, point), normal).abs() > tolerance) {
        return None;
    }
    let u = normalize(cross(normal, if normal[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }))?;
    let w = cross(normal, u);
    let flat: Vec<[f64; 2]> = path.iter().map(|&p| sub(p, point)).map(|d| [dot(d, u), dot(d, w)]).collect();
    let (cx, cy, radius) = crate::shapes::fit_circle(&flat)?;
    let centre = add(point, add(scale(u, cx), scale(w, cy)));
    if path.iter().any(|&p| (length(sub(p, centre)) - radius).abs() > tolerance) {
        return None;
    }
    // How far round: the angles turned between successive points, added up.
    let angle = |p: [f64; 2]| (p[1] - cy).atan2(p[0] - cx);
    let mut sweep = 0.0;
    for pair in flat.windows(2) {
        let mut d = angle(pair[1]) - angle(pair[0]);
        if d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        } else if d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        sweep += d;
    }
    Some(Shape::Circle { centre, axis: normal, radius, sweep: sweep.abs().min(std::f64::consts::TAU) })
}

/// The drawn edge nearest screen point (`x`, `y`) of `frame`, drawn by `camera`, within
/// `reach` pixels and not hidden behind a surface; followed along its run. With the point on it
/// nearest the click. `outlines` and `hidden` are per mesh, as [`crate::Look`] takes them.
#[must_use]
pub fn edge_near(
    meshes: &[crate::WorldMesh],
    outlines: &[Vec<[u32; 2]>],
    hidden: &[bool],
    camera: &Camera,
    frame: &Frame,
    (x, y): (f64, f64),
    reach: f64,
) -> Option<(Edge, [f64; 3])> {
    let (w, h) = (frame.image.width, frame.image.height);
    let mut best: Option<(f64, usize, usize, [f64; 3])> = None;
    for (k, m) in meshes.iter().enumerate() {
        if hidden.get(k).copied().unwrap_or(false) {
            continue;
        }
        let corner = |i: u32| usize::try_from(i).ok().and_then(|i| m.mesh.positions.get(i).copied());
        for (e, &[a, b]) in outlines.get(k).map_or(&[][..], Vec::as_slice).iter().enumerate() {
            let (Some(pa), Some(pb)) = (corner(a), corner(b)) else { continue };
            let (Some(sa), Some(sb)) = (camera.project(pa, w, h), camera.project(pb, w, h)) else { continue };
            let (d, t) = segment_distance_2d([x, y], sa, sb);
            if d > reach || best.is_some_and(|(bd, ..)| d >= bd) {
                continue;
            }
            // Hidden behind a surface nearer than the edge there: not pickable. The point on the
            // edge under the screen point: in perspective, a share of the way along on the
            // screen is a different share in the world (1/depth goes evenly across the screen).
            let t_world = match (camera.projection, camera.basis().ok()) {
                (crate::Projection::Perspective { .. }, Some([_, _, forward])) => {
                    let (za, zb) = (dot(sub(pa, camera.eye), forward), dot(sub(pb, camera.eye), forward));
                    let denominator = (1.0 - t) / za + t / zb;
                    if za > 0.0 && zb > 0.0 && denominator > 0.0 { (t / zb) / denominator } else { t }
                }
                _ => t,
            };
            let on = add(pa, scale(sub(pb, pa), t_world));
            let (px, py) = (sa[0] + (sb[0] - sa[0]) * t, sa[1] + (sb[1] - sa[1]) * t);
            if px < 0.0 || py < 0.0 {
                continue;
            }
            // Inside the picture, so the casts are exact.
            let (sx, sy) = (px as u32, py as u32);
            let seen = frame.point_at(sx, sy, camera);
            let far = |p: [f64; 3]| length(sub(p, camera.eye));
            // The part's own surface beside its edge, sampled a pixel off it, may lie a little
            // nearer on a slant; another part has to be clearly nearer to hide it.
            let slack = if frame.mesh_at(sx, sy) == Some(k) { 2e-2 } else { 2e-3 };
            if seen.is_some_and(|s| far(s) < far(on) * (1.0 - slack)) {
                continue;
            }
            best = Some((d, k, e, on));
        }
    }
    let (_, k, e, on) = best?;
    let mesh = &meshes.get(k)?.mesh;
    Some((Edge { mesh: k, path: follow(mesh, outlines.get(k)?, e) }, on))
}

/// The distance from `p` to the segment `a b` on the screen, and where along it (0 to 1) the
/// nearest point is.
fn segment_distance_2d(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a[0] + dx * t - p[0], a[1] + dy * t - p[1]);
    ((qx * qx + qy * qy).sqrt(), t)
}

/// The run of `mesh`'s outline segments through segment `start`, as points in order: followed
/// both ways while each joint has just the two segments of this edge and turns less than a
/// [`CORNER`].
#[must_use]
pub fn follow(mesh: &TriangleMesh, outlines: &[[u32; 2]], start: usize) -> Vec<[f64; 3]> {
    let pos = |i: u32| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i).copied());
    let key = |i: u32| pos(i).map(|p| p.map(f64::to_bits));
    // Which segments meet at each corner position.
    let mut at: HashMap<[u64; 3], Vec<usize>> = HashMap::new();
    for (s, &[a, b]) in outlines.iter().enumerate() {
        for i in [a, b] {
            if let Some(k) = key(i) {
                at.entry(k).or_default().push(s);
            }
        }
    }
    let Some(&[a, b]) = outlines.get(start) else { return Vec::new() };
    let (Some(pa), Some(pb)) = (pos(a), pos(b)) else { return Vec::new() };
    // Walks on from `here`, having come along segment `came` from `prev`.
    let walk = |mut prev: [f64; 3], mut here_index: u32, mut came: usize| -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        for _ in 0..MAX_SEGMENTS {
            let Some(here) = pos(here_index) else { break };
            let Some(joint) = key(here_index).and_then(|k| at.get(&k)) else { break };
            let next: Vec<usize> = joint.iter().copied().filter(|&s| s != came).collect();
            let [s] = next.as_slice() else { break };
            let Some(&[c, d]) = outlines.get(*s) else { break };
            let far = if key(c) == key(here_index) { d } else { c };
            let Some(there) = pos(far) else { break };
            let turn =
                |u: [f64; 3], v: [f64; 3]| normalize(u).zip(normalize(v)).map_or(std::f64::consts::PI, |(u, v)| dot(u, v).clamp(-1.0, 1.0).acos());
            if turn(sub(here, prev), sub(there, here)) > CORNER || there == pa || there == pb {
                if there == pa || there == pb {
                    // Round a closed loop: end where it began.
                    out.push(there);
                }
                break;
            }
            out.push(there);
            (prev, here_index, came) = (here, far, *s);
        }
        out
    };
    let ahead = walk(pa, b, start);
    if ahead.last() == Some(&pa) {
        // All the way round: the loop from the segment picked back to its start.
        let mut path = vec![pa, pb];
        path.extend(ahead);
        return path;
    }
    let mut behind = walk(pb, a, start);
    behind.reverse();
    let mut path = behind;
    path.extend([pa, pb]);
    path.extend(ahead);
    path.dedup();
    path
}

#[cfg(test)]
mod tests;
