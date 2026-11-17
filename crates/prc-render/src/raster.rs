//! Projection, near-plane clipping and the depth-tested triangle fill behind [`crate::render`].

use crate::render::{Camera, Projection, RenderError};
use crate::vec3::{length, sub};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

/// One view-space corner: x, y, z (z along the view direction), its red, green and blue shade,
/// then how far it lies past the section plane (0 without one).
pub(crate) type ViewCorner = [f64; 7];

/// One screen-space corner: pixel x, pixel y and depth key, and its shade.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Corner {
    pub(crate) at: [f64; 3],
    pub(crate) shade: [f64; 3],
    /// How far it lies past the section plane: positive is cut away.
    pub(crate) cut: f64,
}

/// The perspective near plane, as a fraction of the eye-to-target distance.
pub(crate) const NEAR_FRACTION: f64 = 1e-4;

pub(crate) enum View {
    /// `focal` = 1 / tan(fov_y / 2); `near` = the clip distance.
    Perspective {
        focal: f64,
        aspect: f64,
        near: f64,
    },
    Orthographic {
        half_height: f64,
        aspect: f64,
    },
}

impl View {
    /// The projection for `camera` at width/height `aspect`; refuses a field of view outside
    /// (0, 180) degrees or a non-positive orthographic height.
    pub(crate) fn new(camera: &Camera, aspect: f64) -> Result<View, RenderError> {
        match camera.projection {
            Projection::Perspective { fov_y } => {
                if !(fov_y > 0.0 && fov_y < 180.0) {
                    return Err(RenderError::Camera("the field of view is not between 0 and 180 degrees"));
                }
                let focal = 1.0 / (fov_y.to_radians() / 2.0).tan();
                // A field of view too small to measure leaves no finite focal length.
                if !focal.is_finite() {
                    return Err(RenderError::Camera("the field of view is too small"));
                }
                let distance = length(sub(camera.target, camera.eye));
                Ok(View::Perspective { focal, aspect, near: (distance * NEAR_FRACTION).max(f64::MIN_POSITIVE) })
            }
            Projection::Orthographic { height } => {
                if !(height.is_finite() && height > 0.0) {
                    return Err(RenderError::Camera("the orthographic height is not positive"));
                }
                Ok(View::Orthographic { half_height: height / 2.0, aspect })
            }
        }
    }

    /// The part of `tri` in front of the near plane (Sutherland–Hodgman against one plane):
    /// 0, 3 or 4 corners, the shade interpolated along each cut edge.
    pub(crate) fn clip(&self, tri: [ViewCorner; 3]) -> Clipped {
        let View::Perspective { near, .. } = *self else {
            return Clipped { corners: [tri[0], tri[1], tri[2], tri[2]], len: 3 };
        };
        let mut out = Clipped { corners: [[0.0; 7]; 4], len: 0 };
        for (k, &p) in tri.iter().enumerate() {
            let q = tri.get((k + 1) % 3).copied().unwrap_or(p);
            let z = |v: ViewCorner| v.get(2).copied().unwrap_or(0.0);
            let (p_in, q_in) = (z(p) >= near, z(q) >= near);
            if p_in {
                out.push(p);
            }
            if p_in != q_in {
                // Cut from the same end whichever way round the edge is met, so two triangles
                // sharing it get the same cut point.
                let (s, e) = if first(&p, &q) { (p, q) } else { (q, p) };
                let t = (near - z(s)) / (z(e) - z(s));
                let mut cut = s;
                for (c, d) in cut.iter_mut().zip(e) {
                    *c += t * (d - *c);
                }
                out.push(cut);
            }
        }
        out
    }

    /// The screen corner of view corner `v`: pixel x, pixel y and a depth key that is linear
    /// in screen space and larger for nearer points.
    pub(crate) fn project(&self, [x, y, z, r, g, b, cut]: ViewCorner, width: u32, height: u32) -> Corner {
        let (nx, ny, key) = match *self {
            // Divided before multiplied, so an on-screen point's intermediate products stay finite.
            View::Perspective { focal, aspect, .. } => (x / z * focal / aspect, y / z * focal, 1.0 / z),
            View::Orthographic { half_height, aspect } => (x / half_height / aspect, y / half_height, -z),
        };
        Corner { at: [(nx + 1.0) / 2.0 * f64::from(width), (1.0 - ny) / 2.0 * f64::from(height), key], shade: [r, g, b], cut }
    }
}

/// The corners [`View::clip`] leaves: at most four, as a triangle cut by one plane has.
pub(crate) struct Clipped {
    corners: [ViewCorner; 4],
    len: usize,
}

impl Clipped {
    fn push(&mut self, v: ViewCorner) {
        if let Some(slot) = self.corners.get_mut(self.len) {
            *slot = v;
            self.len += 1;
        }
    }

    /// The corners, in order.
    pub(crate) fn as_slice(&self) -> &[ViewCorner] {
        self.corners.get(..self.len).unwrap_or(&[])
    }
}

/// Whether view corner `a`'s position comes before `b`'s, ordering x, then y, then z.
fn first(a: &ViewCorner, b: &ViewCorner) -> bool {
    a.iter().zip(b).take(3).map(|(p, q)| p.total_cmp(q)).find(|o| o.is_ne()).is_none_or(std::cmp::Ordering::is_lt)
}

/// The image being drawn: `rgba` holds `width * height` straight-RGBA pixels and `depth` their
/// depth keys, row by row from the top.
pub(crate) struct Target<'b> {
    /// Depth keys are 1/z (a perspective view), so the shade is interpolated perspective-correct.
    pub(crate) perspective: bool,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: &'b mut [u8],
    pub(crate) depth: &'b mut [f64],
    /// Per pixel, the id of what it shows ([`NO_ID`] for the background).
    pub(crate) ids: &'b mut [u32],
    /// Pixel centres the fills may still test.
    pub(crate) work: u64,
    /// A work allowance shared with other targets drawing the same image, which [`Self::work`]
    /// draws from when it runs short.
    pub(crate) pool: Option<&'b AtomicU64>,
    /// The image rows this target holds and draws; `rgba` and `depth` start at the first.
    pub(crate) rows: Range<u32>,
}

/// The id of a pixel nothing is drawn at.
pub(crate) const NO_ID: u32 = u32::MAX;

/// How much work a target takes from a shared pool at a time.
const WORK_CHUNK: u64 = 1 << 20;

impl Target<'_> {
    /// Fill the screen triangle of `corners` with straight-RGBA `colour`, depth-tested at pixel
    /// centres, the shade interpolated across it. A pixel centre on an edge two triangles share
    /// belongs to one of them only. An opaque pixel replaces the pixel and its depth; a
    /// translucent one is blended over it, leaving the depth. The clipped bounding box's pixel
    /// centres are charged to [`Self::work`] first: [`RenderError::TooMuchWork`], with nothing
    /// drawn, when too few are left.
    pub(crate) fn fill(&mut self, corners: [Corner; 3], colour: [u8; 4], id: u32, sectioned: bool) -> Result<(), RenderError> {
        let [a, b, c] = corners;
        let area = edge(a.at, b.at, c.at);
        let edges = [(b.at, c.at), (c.at, a.at), (a.at, b.at)];
        if !area.is_finite() || area.abs() < 1e-12 {
            return Ok(());
        }
        let Some((x0, x1, y0, y1)) = self.bounds(&corners) else {
            return Ok(());
        };
        let span = |lo: u32, hi: u32| u64::from(hi.saturating_sub(lo)).saturating_add(1);
        self.charge(span(x0, x1).saturating_mul(span(y0, y1)))?;
        // Only this target's rows are drawn: a band of the image, when it is drawn in bands.
        let (y0, y1) = (y0.max(self.rows.start), y1.min(self.rows.end.saturating_sub(1)));
        let lines = edges.map(|(from, to)| Line::new(from, to, area));
        let flat = (a.shade == b.shade && b.shade == c.shade).then_some(a.shade);
        // Each corner's key, and its shade weighted for interpolation, once per triangle.
        let keys = corners.map(|c| at(c.at, 2));
        // A triangle wholly past the section plane is left out; one partly past is cut pixel by pixel.
        let cuts = corners.map(|c| c.cut);
        if sectioned && cuts.iter().all(|&d| d > 0.0) {
            return Ok(());
        }
        let cut_terms = [0, 1, 2].map(|k| at(cuts, k) * if self.perspective { at(keys, k) } else { 1.0 });
        let partly_cut = sectioned && cuts.iter().any(|&d| d > 0.0);
        let shade_terms = corners.map(|c| [0, 1, 2].map(|j| if self.perspective { at(c.shade, j) * at(c.at, 2) } else { at(c.shade, j) }));
        for y in y0..=y1 {
            let py = f64::from(y) + 0.5;
            let rows = lines.map(|l| l.row(py));
            let Some((xa, xb)) = row_span(&rows, x0, x1) else {
                continue;
            };
            for x in xa..=xb {
                let px = f64::from(x) + 0.5;
                // Each test is the same arithmetic as `edge(from, to, p) / area`, so a pixel is
                // drawn exactly when it would be pixel by pixel over the whole box.
                let w = [0, 1, 2].map(|k| rows.get(k).map_or(0.0, |r| r.weight(px)));
                if (0..3).any(|k| {
                    let wk = w.get(k).copied().unwrap_or(0.0);
                    wk < 0.0 || (wk == 0.0 && !lines.get(k).is_some_and(|l| l.owns))
                }) {
                    continue;
                }
                let mix = |v: [f64; 3]| (-0.0 + at(w, 0) * at(v, 0) + at(w, 1) * at(v, 1)) + at(w, 2) * at(v, 2);
                let key = mix(keys);
                if partly_cut {
                    // Perspective-correct, as the shade is: weights times 1/z, renormalised.
                    let past = if self.perspective && key > 0.0 { mix(cut_terms) / key } else { mix(cuts) };
                    if past > 0.0 {
                        continue;
                    }
                }
                let Some(i) = y.checked_sub(self.rows.start).and_then(|row| pixel_index(self.width, x, row)) else {
                    continue;
                };
                if self.depth.get(i).is_none_or(|&d| key <= d) {
                    continue;
                }
                let shade = match flat {
                    Some(s) => s,
                    None => {
                        let channel_of = |j: usize| mix(shade_terms.map(|t| at(t, j)));
                        if self.perspective && key > 0.0 {
                            // Screen weights times 1/z, renormalised, are the surface's own weights.
                            [0, 1, 2].map(|j| channel_of(j) / key)
                        } else if self.perspective {
                            [0, 1, 2].map(|j| mix(corners.map(|c| at(c.shade, j))))
                        } else {
                            [0, 1, 2].map(channel_of)
                        }
                    }
                };
                self.blend(i, key, shaded(colour, shade));
                if let Some(slot) = self.ids.get_mut(i) {
                    *slot = id;
                }
            }
        }
        Ok(())
    }

    /// Take `cost` from [`Self::work`], topping it up from [`Self::pool`] when short.
    fn charge(&mut self, cost: u64) -> Result<(), RenderError> {
        if let Some(rest) = self.work.checked_sub(cost) {
            self.work = rest;
            return Ok(());
        }
        let needed = cost - self.work;
        let pool = self.pool.ok_or(RenderError::TooMuchWork)?;
        let mut left = pool.load(Ordering::Relaxed);
        loop {
            if left < needed {
                return Err(RenderError::TooMuchWork);
            }
            let taken = needed.max(WORK_CHUNK).min(left);
            match pool.compare_exchange_weak(left, left - taken, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => {
                    // `taken` covers what `work` lacked, so this doesn't underflow.
                    self.work = taken - needed;
                    return Ok(());
                }
                Err(now) => left = now,
            }
        }
    }

    /// The pixel rows and columns `corners` may cover, clipped to the image; `None` when off it.
    fn bounds(&self, corners: &[Corner; 3]) -> Option<(u32, u32, u32, u32)> {
        let lo = |i: usize| corners.iter().map(|c| at(c.at, i)).fold(f64::INFINITY, f64::min).floor().max(0.0);
        let hi = |i: usize, limit: u32| corners.iter().map(|c| at(c.at, i)).fold(f64::NEG_INFINITY, f64::max).ceil().min(f64::from(limit) - 1.0);
        let (x0, x1) = (lo(0), hi(0, self.width));
        let (y0, y1) = (lo(1), hi(1, self.height));
        // Each bound is finite and within 0..limit here, so the casts are exact.
        (x0 <= x1 && y0 <= y1).then_some((x0 as u32, x1 as u32, y0 as u32, y1 as u32))
    }

    /// Write straight-RGBA `rgba` at pixel `i`: replacing it and its depth when opaque, blended
    /// over it otherwise.
    fn blend(&mut self, i: usize, key: f64, rgba: [u8; 4]) {
        let opaque = rgba[3] == 255;
        if opaque && let Some(d) = self.depth.get_mut(i) {
            *d = key;
        }
        let Some(px) = i.checked_mul(4).and_then(|s| self.rgba.get_mut(s..s.checked_add(4)?)) else {
            return;
        };
        if opaque {
            px.copy_from_slice(&rgba);
        } else if let [pr, pg, pb, pa] = px {
            let [sr, sg, sb, sa] = rgba;
            let src = f64::from(sa) / 255.0;
            let dst = f64::from(*pa) / 255.0 * (1.0 - src);
            let out = src + dst;
            if out <= 0.0 {
                return;
            }
            for (p, s) in [pr, pg, pb].into_iter().zip([sr, sg, sb]) {
                *p = channel((f64::from(s) * src + f64::from(*p) * dst) / out);
            }
            *pa = channel(out * 255.0);
        }
    }
}

/// Pixel `x, y`'s index in an image `width` wide; `None` on overflow.
pub(crate) fn pixel_index(width: u32, x: u32, y: u32) -> Option<usize> {
    usize::try_from(y).ok()?.checked_mul(usize::try_from(width).ok()?)?.checked_add(usize::try_from(x).ok()?)
}

/// `v` rounded and clamped to a colour channel; NaN is 0.
pub(crate) fn channel(v: f64) -> u8 {
    // Clamped to 0..=255 first, so the cast is exact (NaN casts to 0).
    v.round().clamp(0.0, 255.0) as u8
}

/// `colour`'s RGB scaled channel by channel by `shade`, saturating at 255; alpha kept.
fn shaded([r, g, b, a]: [u8; 4], [sr, sg, sb]: [f64; 3]) -> [u8; 4] {
    let [r, g, b] = [(r, sr), (g, sg), (b, sb)].map(|(v, s)| channel(f64::from(v) * s));
    [r, g, b, a]
}

/// Twice the signed area of (a, b, p) in the xy plane.
pub(crate) fn edge(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    // Two triangles sharing an edge meet it in opposite orders; evaluating it in one canonical
    // order makes their values exact negatives, so a pixel centre on the edge cannot round out
    // of both and leave a crack.
    if (at(a, 0), at(a, 1)) > (at(b, 0), at(b, 1)) {
        return -edge(b, a, p);
    }
    (at(b, 0) - at(a, 0)) * (at(p, 1) - at(a, 1)) - (at(b, 1) - at(a, 1)) * (at(p, 0) - at(a, 0))
}

/// One edge of a triangle being filled, set up once so each pixel's test is cheap: the same
/// canonical order as [`edge`], so its values are the ones [`edge`] gives.
#[derive(Clone, Copy)]
struct Line {
    /// The canonical start point, x and y.
    ax: f64,
    ay: f64,
    /// `b - a`, canonical order.
    dx: f64,
    dy: f64,
    /// -1 when the canonical order reverses the edge, else 1.
    sign: f64,
    area: f64,
    /// Whether a pixel centre exactly on the edge is the triangle's ([`owns`]).
    owns: bool,
}

/// A [`Line`] on one pixel row: what is left of [`edge`] once y is fixed.
#[derive(Clone, Copy)]
struct RowLine {
    line: Line,
    /// `dx * (py - ay)`.
    across: f64,
}

impl Line {
    fn new(from: [f64; 3], to: [f64; 3], area: f64) -> Line {
        let owns = owns(from, to, area);
        let ((a, b), sign) = if (at(from, 0), at(from, 1)) > (at(to, 0), at(to, 1)) { ((to, from), -1.0) } else { ((from, to), 1.0) };
        Line { ax: at(a, 0), ay: at(a, 1), dx: at(b, 0) - at(a, 0), dy: at(b, 1) - at(a, 1), sign, area, owns }
    }

    fn row(self, py: f64) -> RowLine {
        RowLine { line: self, across: self.dx * (py - self.ay) }
    }
}

impl RowLine {
    /// The corner weight this edge gives pixel centre `px` on the row: `edge(from, to, p) / area`.
    fn weight(&self, px: f64) -> f64 {
        let l = self.line;
        let e = self.across - l.dy * (px - l.ax);
        // `edge` negates the canonical value when it swapped the ends.
        (if l.sign < 0.0 { -e } else { e }) / l.area
    }

    /// The pixel centres on the row where the weight may be at least 0, as a closed range of x
    /// (unbounded when the edge doesn't limit x); padded, as the exact test decides.
    fn reach(&self) -> (f64, f64) {
        let l = self.line;
        // weight(px) = s * (across - dy * (px - ax)), with s = sign / area.
        let slope = -l.dy * l.sign / l.area;
        let at_ax = self.across * l.sign / l.area;
        if slope == 0.0 || !slope.is_finite() {
            return if at_ax >= 0.0 || !at_ax.is_finite() { (f64::NEG_INFINITY, f64::INFINITY) } else { (f64::INFINITY, f64::NEG_INFINITY) };
        }
        let root = l.ax - at_ax / slope;
        if slope > 0.0 { (root, f64::INFINITY) } else { (f64::NEG_INFINITY, root) }
    }
}

/// The pixels `x0..=x1` of a row that may lie inside all three edges, or `None` when none can.
/// Each end is widened by two pixels against rounding; the exact per-pixel test decides.
fn row_span(rows: &[RowLine; 3], x0: u32, x1: u32) -> Option<(u32, u32)> {
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for r in rows {
        let (a, b) = r.reach();
        lo = lo.max(a);
        hi = hi.min(b);
    }
    if lo.is_nan() || hi.is_nan() {
        return Some((x0, x1));
    }
    // Pixel x's centre is x + 0.5.
    let first = (lo - 0.5).floor() - 2.0;
    let last = (hi - 0.5).ceil() + 2.0;
    if last < f64::from(x0) || first > f64::from(x1) || first > last {
        return None;
    }
    // Clamped into x0..=x1 first, so the casts are exact.
    Some((first.max(f64::from(x0)) as u32, last.min(f64::from(x1)) as u32))
}

/// Whether a pixel centre exactly on the edge `from`-`to` of a triangle of signed doubled area
/// `area` is the triangle's: the side the triangle grows towards, +x first, then +y. Two
/// triangles sharing the edge lie on opposite sides of it, so exactly one owns it.
fn owns(from: [f64; 3], to: [f64; 3], area: f64) -> bool {
    let s = area.signum();
    let (gx, gy) = (-(at(to, 1) - at(from, 1)) * s, (at(to, 0) - at(from, 0)) * s);
    gx > 0.0 || (gx == 0.0 && gy > 0.0)
}

/// Component `i` of `v`, 0.0 out of range.
pub(crate) fn at(v: [f64; 3], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_seen_from_either_triangle_is_exactly_negated() {
        // So no pixel centre on a shared edge is rejected by both.
        let vals = [0.1, 0.7, 1.3, 2.9, 3.3, 5.5];
        for (a, b, p) in [([0.1, 0.1], [0.7, 0.7], [0.1, 2.9]), ([5.5, 0.7], [1.3, 3.3], [2.9, 0.1])]
            .into_iter()
            .chain(vals.iter().map(|&v| ([v, 0.1], [0.7, v], [v, v])))
        {
            let [a, b, p] = [a, b, p].map(|[x, y]| [x, y, 0.0]);
            assert_eq!(edge(a, b, p), -edge(b, a, p), "{a:?} {b:?} {p:?}");
        }
    }

    #[test]
    fn pixel_indices_and_channels_never_overflow() {
        assert_eq!(pixel_index(10, 3, 2), Some(23));
        assert_eq!(channel(f64::NAN), 0);
        assert_eq!(channel(1e300), 255);
        assert_eq!(channel(-4.0), 0);
        assert_eq!(channel(127.5), 128);
    }

    #[test]
    fn a_shared_edge_is_cut_at_the_same_point_from_either_triangle() {
        // Cutting this edge from P towards Q and from Q towards P rounds differently.
        let view = View::Perspective { focal: 1.0, aspect: 1.0, near: 0.5 };
        let (p, q) = ([0.1, 0.1, 0.2, 1.0, 1.0, 1.0, 0.0], [0.7, 0.2, 1.6, 1.0, 1.0, 1.0, 0.0]);
        let one = view.clip([p, q, [0.0, 3.0, 2.0, 1.0, 1.0, 1.0, 0.0]]);
        let one = one.as_slice();
        let other = view.clip([q, p, [3.0, 0.0, 2.0, 1.0, 1.0, 1.0, 0.0]]);
        let other = other.as_slice();
        let shared = one.iter().filter(|a| other.iter().any(|b| a[..3] == b[..3])).count();
        assert_eq!(shared, 2, "Q and the cut on PQ: {one:?} {other:?}");
    }

    fn corner(x: f64, y: f64, key: f64, shade: f64) -> Corner {
        Corner { at: [x, y, key], shade: [shade; 3], cut: 0.0 }
    }

    #[test]
    fn a_pixel_on_an_edge_two_triangles_share_is_blended_once() {
        let (mut rgba, mut depth) = (vec![0; 64], vec![f64::NEG_INFINITY; 16]);
        let mut t = Target {
            perspective: false,
            width: 4,
            height: 4,
            rgba: &mut rgba,
            depth: &mut depth,
            ids: &mut [NO_ID; 16],
            work: u64::MAX,
            pool: None,
            rows: 0..4,
        };
        let [a, b, c, d] = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)].map(|(x, y)| corner(x, y, 1.0, 1.0));
        t.fill([a, b, c], [255, 0, 0, 128], 0, false).unwrap();
        t.fill([a, c, d], [255, 0, 0, 128], 0, false).unwrap();
        // The diagonal runs through four pixel centres; each must look like the rest.
        for px in rgba.chunks(4) {
            assert_eq!(px, [255, 0, 0, 128]);
        }
    }

    #[test]
    fn a_perspective_shade_is_interpolated_on_the_surface_not_the_screen() {
        let (mut rgba, mut depth) = (vec![0; 64], vec![f64::NEG_INFINITY; 16]);
        let mut t = Target {
            perspective: true,
            width: 4,
            height: 4,
            rgba: &mut rgba,
            depth: &mut depth,
            ids: &mut [NO_ID; 16],
            work: u64::MAX,
            pool: None,
            rows: 0..4,
        };
        // The x = 4 corner is three times as far away as the others, and the only lit one.
        t.fill([corner(0.0, 0.0, 1.0, 0.0), corner(4.0, 0.0, 1.0 / 3.0, 1.0), corner(0.0, 4.0, 1.0, 0.0)], [240, 240, 240, 255], 0, false).unwrap();
        // At screen (1.5, 0.5) the weights are 0.5, 0.375, 0.125: on the surface the far corner
        // counts 0.125 / 0.75 = 1/6, where on the screen it would count 0.375.
        assert_eq!(rgba.get(4..8), Some(&[40, 40, 40, 255][..]));
    }

    #[test]
    fn fills_stop_when_the_work_allowance_is_spent() {
        let (mut rgba, mut depth) = (vec![0; 64], vec![f64::NEG_INFINITY; 16]);
        // Each fill's box is the whole 4x4 image: 16 pixel centres.
        let mut t = Target {
            perspective: false,
            width: 4,
            height: 4,
            rgba: &mut rgba,
            depth: &mut depth,
            ids: &mut [NO_ID; 16],
            work: 40,
            pool: None,
            rows: 0..4,
        };
        let tri = [corner(0.0, 0.0, 1.0, 1.0), corner(4.0, 0.0, 1.0, 1.0), corner(0.0, 4.0, 1.0, 1.0)];
        assert_eq!(t.fill(tri, [255, 0, 0, 255], 0, false), Ok(()));
        assert_eq!(t.fill(tri, [255, 0, 0, 255], 0, false), Ok(()));
        assert_eq!(t.fill(tri, [255, 0, 0, 255], 0, false), Err(RenderError::TooMuchWork));
        assert_eq!(t.work, 8);
    }
}
