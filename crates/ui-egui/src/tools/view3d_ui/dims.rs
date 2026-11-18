//! Dimension layers: measurements kept on one part (or sub-assembly), drawn as a drawing draws
//! them and saved in the PDF with the 3D annotation (`pdfcraft_annot::Layer3d`). A layer belongs
//! to the part by name, in the part's own coordinates, so every copy of the part shows it; it is
//! shown only while its part is isolated.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{Dim3d, DimKind3d, Layer3d, LayerKind3d, LayerView3d};
use pdfcraft_prc_render::measure::Reading;
use pdfcraft_prc_render::{Camera, Projection};

use super::measure::Units;
use super::scene::{Scene, add, cross, dot, normalize, scale, sub};

/// Dimension lines and text.
const LINE: Color32 = Color32::from_rgb(0x10, 0x10, 0x14);
/// The dimension picked, to set its tolerance or delete it.
const PICKED: Color32 = Color32::from_rgb(0xE0, 0x6A, 0x00);
/// The bar naming the part a layer belongs to.
const BAR: Color32 = Color32::from_rgb(0x1B, 0x63, 0xE0);

/// A part's own coordinates: world = `m` × local.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PartFrame {
    m: [[f64; 4]; 4],
    inverse: [[f64; 4]; 4],
}

impl PartFrame {
    pub(crate) const IDENTITY: PartFrame = PartFrame { m: IDENTITY, inverse: IDENTITY };

    pub(crate) fn new(m: [[f64; 4]; 4]) -> Option<PartFrame> {
        Some(PartFrame { m, inverse: inverse(&m)? })
    }

    pub(crate) fn world_of(self, p: [f64; 3]) -> [f64; 3] {
        apply(&self.m, p, 1.0)
    }

    pub(crate) fn local_of(self, p: [f64; 3]) -> [f64; 3] {
        apply(&self.inverse, p, 1.0)
    }

    fn dir_to_world(self, d: [f64; 3]) -> [f64; 3] {
        apply(&self.m, d, 0.0)
    }

    fn dir_to_local(self, d: [f64; 3]) -> [f64; 3] {
        apply(&self.inverse, d, 0.0)
    }

    /// How much longer lengths are in the world than in the part (1 for a rigid placement).
    fn stretch(&self) -> f64 {
        let x = self.dir_to_world([1.0, 0.0, 0.0]);
        dot(x, x).sqrt()
    }

    /// `camera` as a layer saves it, in the part's coordinates.
    pub(crate) fn save_view(&self, camera: &Camera) -> LayerView3d {
        let (ortho_height, fov_y) = match camera.projection {
            Projection::Orthographic { height } => (Some(height / self.stretch().max(1e-12)), 30.0),
            Projection::Perspective { fov_y } => (None, fov_y),
        };
        LayerView3d { eye: self.local_of(camera.eye), target: self.local_of(camera.target), up: self.dir_to_local(camera.up), ortho_height, fov_y }
    }

    /// A layer's saved view as a camera, placed with this copy of the part.
    pub(crate) fn camera(&self, v: &LayerView3d) -> Camera {
        let projection = match v.ortho_height {
            Some(h) => Projection::Orthographic { height: h * self.stretch() },
            None => Projection::Perspective { fov_y: v.fov_y },
        };
        Camera { eye: self.world_of(v.eye), target: self.world_of(v.target), up: self.dir_to_world(v.up), projection }
    }
}

const IDENTITY: [[f64; 4]; 4] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

fn apply(m: &[[f64; 4]; 4], p: [f64; 3], w: f64) -> [f64; 3] {
    let row = |r: &[f64; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3] * w;
    [row(&m[0]), row(&m[1]), row(&m[2])]
}

/// The inverse of an affine `m` (its last row 0 0 0 1), or `None` when it flattens space.
#[allow(clippy::needless_range_loop)]
fn inverse(m: &[[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    let a = |r: usize, c: usize| m[r][c];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    if !det.is_finite() || det.abs() < 1e-15 {
        return None;
    }
    let mut out = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            // The cofactor of (c, r), over the determinant.
            let (r1, r2) = ((c + 1) % 3, (c + 2) % 3);
            let (c1, c2) = ((r + 1) % 3, (r + 2) % 3);
            out[r][c] = (a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)) / det;
        }
    }
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    for r in 0..3 {
        out[r][3] = -(out[r][0] * t[0] + out[r][1] * t[1] + out[r][2] * t[2]);
    }
    Some(out)
}

/// The part `node` stands for, as layers name it: its name without the instance number.
pub(crate) fn part_name(scene: &Scene, node: usize) -> String {
    let name = scene.nodes.get(node).map_or("", |n| n.name.as_str());
    match name.rsplit_once('-') {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base.to_owned(),
        _ => name.to_owned(),
    }
}

/// How many copies of `part` the assembly has.
pub(crate) fn copies(scene: &Scene, part: &str) -> usize {
    (0..scene.nodes.len()).filter(|&k| part_name(scene, k) == part).count()
}

/// `node`'s own coordinates: those of the first mesh under it, which every copy of it shares.
pub(crate) fn frame_of(scene: &Scene, node: usize) -> PartFrame {
    let mut meshes: Vec<usize> = scene.subtree(node).into_iter().filter_map(|k| scene.nodes.get(k)).flat_map(|n| n.meshes.iter().copied()).collect();
    meshes.sort_unstable();
    meshes.into_iter().find_map(|k| scene.placed_by.get(k).copied().flatten()).and_then(PartFrame::new).unwrap_or(PartFrame::IDENTITY)
}

/// A reading kept as a dimension on a part seen through `camera`, in the part's coordinates.
pub(crate) fn dim_of(r: &Reading, frame: &PartFrame, camera: &Camera, units: Units) -> Dim3d {
    let places = if (units.mm_per_unit - 25.4).abs() < 1e-9 { 3 } else { 2 };
    let forward = normalize(sub(camera.target, camera.eye)).unwrap_or([0.0, 0.0, -1.0]);
    let (kind, value, points, text_at, exact) = match *r {
        Reading::Distance { value, from, to, exact } => {
            // Text beside the middle, square to the measured line and to the view.
            let side = normalize(cross(sub(to, from), forward)).unwrap_or(camera.up);
            let middle = scale(add(from, to), 0.5);
            (DimKind3d::Distance, value, vec![from, to], add(middle, scale(side, (value * 0.25).max(1e-6))), exact)
        }
        Reading::Diameter { value, centre, at } => (DimKind3d::Diameter, value, vec![centre, at], add(at, scale(sub(at, centre), 0.6)), true),
        Reading::Radius { value, centre, at } => (DimKind3d::Radius, value, vec![centre, at], add(at, scale(sub(at, centre), 0.6)), true),
        Reading::Angle { degrees, at } => (DimKind3d::Angle, degrees, vec![at], add(at, scale(camera.up, 0.05)), true),
        Reading::ArcLength { value, at } => (DimKind3d::ArcLength, value, vec![at], add(at, scale(camera.up, value * 0.3)), true),
    };
    Dim3d {
        kind,
        value,
        points: points.into_iter().map(|p| frame.local_of(p)).collect(),
        text_at: frame.local_of(text_at),
        plus: 0.0,
        minus: 0.0,
        places: if kind == DimKind3d::Angle { 1 } else { places },
        exact,
    }
}

/// A dimension's text as a drawing shows it, in the model's unit only.
pub(crate) fn dim_text(d: &Dim3d, units: Units) -> String {
    let inch = (units.mm_per_unit - 25.4).abs() < 1e-9;
    let places = usize::from(d.places.min(6));
    let unit_value = |v: f64| if inch { v * units.mm_per_unit / 25.4 } else { v * units.mm_per_unit };
    let number = |v: f64| format!("{:.*}", places, v);
    let main = match d.kind {
        DimKind3d::Angle => format!("{}°", number(d.value)),
        DimKind3d::Diameter => format!("Ø{}", number(unit_value(d.value))),
        DimKind3d::Radius => format!("R{}", number(unit_value(d.value))),
        DimKind3d::Distance | DimKind3d::ArcLength => {
            let approx = if d.exact { "" } else { "≈" };
            format!("{approx}{}", number(unit_value(d.value)))
        }
    };
    let tol = |v: f64| if d.kind == DimKind3d::Angle { number(v) } else { number(unit_value(v)) };
    if d.plus == 0.0 && d.minus == 0.0 {
        main
    } else if (d.plus - d.minus).abs() < 1e-12 {
        format!("{main} ±{}", tol(d.plus))
    } else {
        format!("{main} +{}/−{}", tol(d.plus), tol(d.minus))
    }
}

/// The dimension layer for `part`, if it has one.
pub(crate) fn layer_for(layers: &[Layer3d], part: &str) -> Option<usize> {
    layers.iter().position(|l| l.part == part && l.kind == LayerKind3d::Dimensions)
}

/// What [`paint`] draws.
pub(crate) struct Shown<'a> {
    pub(crate) layer: &'a Layer3d,
    /// The layer is the one being worked on: its dimensions can be picked and moved.
    pub(crate) active: bool,
    pub(crate) picked: Option<usize>,
    pub(crate) copies: usize,
}

/// Draws a layer's dimensions over the model shown in `r` (a picture `size` pixels big drawn by
/// `camera`), with a bar naming its part. Where each dimension's text is, for picking.
pub(crate) fn paint(
    painter: &egui::Painter,
    r: Rect,
    size: [u32; 2],
    camera: &Camera,
    frame: &PartFrame,
    shown: &Shown<'_>,
    units: Units,
) -> Vec<(usize, Rect)> {
    let to_screen = |p: [f64; 3]| -> Option<Pos2> {
        let [x, y] = camera.project(frame.world_of(p), size[0], size[1])?;
        let (w, h) = (f64::from(size[0].max(1)), f64::from(size[1].max(1)));
        Some(pos2(r.left() + (x / w) as f32 * r.width(), r.top() + (y / h) as f32 * r.height()))
    };
    let mut hits = Vec::new();
    for (k, d) in shown.layer.dims.iter().enumerate() {
        let colour = if shown.picked == Some(k) { PICKED } else { LINE };
        let stroke = Stroke::new(1.2, colour);
        let points: Vec<Pos2> = d.points.iter().filter_map(|&p| to_screen(p)).collect();
        let Some(text_at) = to_screen(d.text_at) else { continue };
        match (d.kind, points.as_slice()) {
            (DimKind3d::Distance, &[a, b]) => {
                // Extension lines out to the dimension line through the text, arrows at both
                // ends of it.
                let along = (b - a).normalized();
                if along.is_finite() && along.length() > 0.5 {
                    let across = vec2(-along.y, along.x);
                    let off = across * (text_at - a).dot(across);
                    let (da, db) = (a + off, b + off);
                    let gap = off.normalized() * 3.0;
                    let over = off.normalized() * 4.0;
                    if off.length() > 4.0 {
                        painter.line_segment([a + gap, da + over], stroke);
                        painter.line_segment([b + gap, db + over], stroke);
                    }
                    painter.line_segment([da, db], stroke);
                    arrow(painter, da, -along, colour);
                    arrow(painter, db, along, colour);
                    // The dimension line runs on to text placed past its ends.
                    let t = (text_at - da).dot(along);
                    let length = (db - da).length();
                    if t < 0.0 {
                        painter.line_segment([da, da + along * t], stroke);
                    } else if t > length {
                        painter.line_segment([db, da + along * t], stroke);
                    }
                }
            }
            (DimKind3d::Diameter, &[c, at]) => {
                // Across the round through its centre, arrows on the far sides.
                let dir = (at - c).normalized();
                if dir.is_finite() {
                    let far = c - (at - c);
                    painter.line_segment([far, at], stroke);
                    arrow(painter, at, dir, colour);
                    arrow(painter, far, -dir, colour);
                    leader(painter, at, text_at, stroke);
                }
            }
            (DimKind3d::Radius, &[c, at]) => {
                let dir = (at - c).normalized();
                if dir.is_finite() {
                    painter.line_segment([c, at], stroke);
                    painter.circle_filled(c, 1.8, colour);
                    arrow(painter, at, dir, colour);
                    leader(painter, at, text_at, stroke);
                }
            }
            (_, &[at, ..]) => leader(painter, at, text_at, stroke),
            _ => {}
        }
        let text = dim_text(d, units);
        let galley = painter.layout_no_wrap(text, crate::theme::medium(13.0), colour);
        let rect = Rect::from_center_size(text_at, galley.size());
        // A light ground, so the text reads over any colour of part.
        painter.rect_filled(rect.expand(2.0), CornerRadius::same(2), Color32::from_white_alpha(225));
        if shown.active && shown.picked == Some(k) {
            painter.rect_stroke(rect.expand(3.0), CornerRadius::same(2), Stroke::new(1.0, PICKED), egui::StrokeKind::Outside);
        }
        painter.galley(rect.min, galley, colour);
        hits.push((k, rect.expand(3.0)));
    }
    // Which part the layer is on, across the top of the view.
    let mut text = crate::i18n::fmt(tl!("Dimensions: {part}"), &[("part", &shown.layer.part)]);
    if shown.copies > 1 {
        text.push_str(" · ");
        text.push_str(&crate::i18n::fmt(tl!("shared by {n} copies"), &[("n", &shown.copies.to_string())]));
    }
    let galley = painter.layout_no_wrap(text, crate::theme::semibold(12.0), Color32::WHITE);
    let at = pos2(r.center().x - galley.size().x / 2.0, r.top() + 8.0);
    painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(vec2(8.0, 4.0)), CornerRadius::same(4), BAR);
    painter.galley(at, galley, Color32::WHITE);
    hits
}

/// An arrowhead with its point at `tip`, pointing along unit `dir`.
fn arrow(painter: &egui::Painter, tip: Pos2, dir: egui::Vec2, colour: Color32) {
    let back = tip - dir * 9.0;
    let side = vec2(-dir.y, dir.x) * 3.0;
    painter.add(egui::Shape::convex_polygon(vec![tip, back + side, back - side], colour, Stroke::NONE));
}

/// A leader from `from` to the text at `to`, with a short shoulder into the text.
fn leader(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let shoulder = if to.x >= from.x { -14.0 } else { 14.0 };
    let knee = pos2(to.x + shoulder, to.y);
    painter.line_segment([from, knee], stroke);
    painter.line_segment([knee, to], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_frame_goes_to_the_world_and_back() {
        // Turned a quarter about z and moved.
        let m = [[0.0, -1.0, 0.0, 5.0], [1.0, 0.0, 0.0, 2.0], [0.0, 0.0, 1.0, -1.0], [0.0, 0.0, 0.0, 1.0]];
        let f = PartFrame::new(m).unwrap();
        let p = [1.0, 2.0, 3.0];
        assert_eq!(f.world_of(p), [3.0, 3.0, 2.0]);
        let back = f.local_of(f.world_of(p));
        assert!((0..3).all(|k| (back[k] - p[k]).abs() < 1e-12), "{back:?}");
        assert!(PartFrame::new([[0.0; 4]; 4]).is_none());
        // A saved view comes back where it was.
        let camera =
            Camera { eye: [9.0, 1.0, 4.0], target: [5.0, 2.0, -1.0], up: [0.0, 0.0, 1.0], projection: Projection::Orthographic { height: 3.0 } };
        let again = f.camera(&f.save_view(&camera));
        assert!((0..3).all(|k| (again.eye[k] - camera.eye[k]).abs() < 1e-12 && (again.up[k] - camera.up[k]).abs() < 1e-12));
        assert_eq!(again.projection, camera.projection);
    }

    #[test]
    fn dimensions_read_as_a_drawing_writes_them() {
        let inch = Units { mm_per_unit: 25.4, stated: true };
        let d = Dim3d { kind: DimKind3d::Diameter, value: 0.375, points: vec![], text_at: [0.0; 3], plus: 0.0, minus: 0.0, places: 3, exact: true };
        assert_eq!(dim_text(&d, inch), "Ø0.375");
        assert_eq!(dim_text(&Dim3d { plus: 0.005, minus: 0.005, ..d.clone() }, inch), "Ø0.375 ±0.005");
        assert_eq!(dim_text(&Dim3d { plus: 0.005, minus: 0.002, kind: DimKind3d::Radius, ..d.clone() }, inch), "R0.375 +0.005/−0.002");
        let mm = Units { mm_per_unit: 1.0, stated: true };
        assert_eq!(dim_text(&Dim3d { kind: DimKind3d::Distance, value: 12.0, places: 2, exact: false, ..d.clone() }, mm), "≈12.00");
        assert_eq!(dim_text(&Dim3d { kind: DimKind3d::Angle, value: 90.0, places: 1, ..d }, inch), "90.0°");
    }

    #[test]
    fn parts_are_named_without_their_instance_numbers() {
        use super::super::scene::Node;
        let node = |name: &str| Node { name: name.into(), ..Node::default() };
        let scene =
            Scene { nodes: vec![node("Threaded Insert Brass 10-32-1"), node("Threaded Insert Brass 10-32-4"), node("Lid")], ..Scene::default() };
        assert_eq!(part_name(&scene, 0), "Threaded Insert Brass 10-32");
        assert_eq!(copies(&scene, "Threaded Insert Brass 10-32"), 2);
        assert_eq!(part_name(&scene, 2), "Lid");
    }
}
