//! The Measure tool on a 3D model: clicks pick faces, whose surfaces are found again from their
//! triangles (`pdfcraft_prc_render::shapes`), and what lies between them is drawn on the model
//! and listed in the Model Tree panel, in the model's units.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_prc_render::measure::{Pick, Picked, Reading, arc_length, between, face_at, picked, single};
use pdfcraft_prc_render::{Camera, WorldMesh};

use super::scene::Scene;

/// The measuring colour: drawn over any model colour.
const INK: Color32 = Color32::from_rgb(0xE0, 0x1B, 0x8A);

/// One model's measurement: the faces picked (one or two) and what they read.
#[derive(Clone, Debug, Default)]
pub(crate) struct Measure {
    pub(crate) picks: Vec<Picked>,
    pub(crate) readings: Vec<Reading>,
    /// The edges picked, as the points along them, drawn over the model.
    pub(crate) edges: Vec<Vec<[f64; 3]>>,
    /// Per reading, where the user dragged its label: from the point it measures, on screen.
    pub(crate) moved: Vec<Option<egui::Vec2>>,
}

impl Measure {
    /// A click on the model at world point `point`, on mesh `mesh`: the face there is picked,
    /// starting again after two.
    pub(crate) fn pick(&mut self, meshes: &[WorldMesh], mesh: usize, point: [f64; 3]) {
        self.start_again_after_two();
        let face = meshes.get(mesh).and_then(|m| face_at(&m.mesh, point));
        self.picks.push(picked(meshes, Pick { mesh, face, point }));
        self.read();
    }

    /// A click on an edge line at `point`: the edge is picked, as a face would be.
    pub(crate) fn pick_edge(&mut self, edge: &pdfcraft_prc_render::edges::Edge, point: [f64; 3]) {
        self.start_again_after_two();
        self.picks.push(pdfcraft_prc_render::measure::picked_edge(edge, point));
        // The piece measured: one straight or one arc.
        self.edges.push(edge.piece(point).path);
        self.read();
    }

    fn start_again_after_two(&mut self) {
        if self.picks.len() >= 2 {
            self.clear();
        }
    }

    fn read(&mut self) {
        self.readings = match self.picks.as_slice() {
            [a] => single(a).into_iter().chain(arc_length(a)).collect(),
            [a, b] => {
                let mut out = between(a, b);
                // Each hole's own size too, as a drawing gives both.
                out.extend(single(a));
                out.extend(single(b));
                out
            }
            _ => Vec::new(),
        };
        self.moved = vec![None; self.readings.len()];
    }

    pub(crate) fn clear(&mut self) {
        self.picks.clear();
        self.readings.clear();
        self.edges.clear();
        self.moved.clear();
    }
}

/// The model's unit, for showing lengths: millimetres per unit, and whether the file says so
/// (else the unit is assumed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Units {
    pub(crate) mm_per_unit: f64,
    pub(crate) stated: bool,
}

impl Units {
    pub(crate) fn of(scene: &Scene) -> Units {
        scene.units.map_or(Units { mm_per_unit: 1.0, stated: false }, |u| Units { mm_per_unit: u.mm_per_unit, stated: u.from_cad_file })
    }

    /// `value` model units as the model's own unit, with the other in brackets: inches with
    /// millimetres for an inch model, else millimetres with inches.
    pub(crate) fn length(&self, value: f64) -> String {
        let mm = value * self.mm_per_unit;
        let inches = mm / 25.4;
        let assumed = if self.stated { "" } else { "?" };
        if (self.mm_per_unit - 25.4).abs() < 1e-9 {
            format!("{inches:.3} in{assumed} ({mm:.2} mm)")
        } else {
            format!("{mm:.2} mm{assumed} ({inches:.3} in)")
        }
    }
}

/// A reading as the panel and the label say it: "Ø 0.750 in (19.05 mm)", "≈ 2.100 in …".
pub(crate) fn text(r: &Reading, units: Units) -> String {
    match *r {
        Reading::Diameter { value, .. } => format!("Ø {}", units.length(value)),
        Reading::Radius { value, .. } => format!("R {}", units.length(value)),
        Reading::Distance { value, exact: true, .. } => units.length(value),
        Reading::Distance { value, exact: false, .. } => format!("≈ {}", units.length(value)),
        Reading::Angle { degrees, .. } => format!("{degrees:.2}°"),
        Reading::ArcLength { value, .. } => units.length(value),
    }
}

/// What a reading is, for the panel.
pub(crate) fn kind(r: &Reading) -> &'static str {
    match r {
        Reading::Diameter { .. } => tl!("Diameter"),
        Reading::Radius { .. } => tl!("Radius"),
        Reading::Distance { exact: true, .. } => tl!("Distance"),
        Reading::Distance { exact: false, .. } => tl!("Distance between the points picked"),
        Reading::Angle { .. } => tl!("Angle"),
        Reading::ArcLength { .. } => tl!("Arc length"),
    }
}

/// Draws `m`'s picks and readings over the model shown in `r`, drawn by `camera` into a picture
/// `size` pixels big. Where each reading's label is, for dragging it.
pub(crate) fn paint(painter: &egui::Painter, r: Rect, size: [u32; 2], camera: &Camera, m: &Measure, units: Units) -> Vec<(usize, Rect)> {
    let to_screen = |p: [f64; 3]| -> Option<Pos2> {
        let [x, y] = camera.project(p, size[0], size[1])?;
        let (w, h) = (f64::from(size[0].max(1)), f64::from(size[1].max(1)));
        Some(pos2(r.left() + (x / w) as f32 * r.width(), r.top() + (y / h) as f32 * r.height()))
    };
    let stroke = Stroke::new(2.0, INK);
    // Picked edges, along their length.
    for path in &m.edges {
        let points: Vec<Pos2> = path.iter().filter_map(|&p| to_screen(p)).collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, Stroke::new(3.0, INK)));
        }
    }
    for p in &m.picks {
        if let Some(at) = to_screen(p.pick.point) {
            painter.circle(at, 4.0, Color32::WHITE, stroke);
        }
    }
    let mut hits = Vec::new();
    for (k, reading) in m.readings.iter().enumerate() {
        let (from, to, label_at) = match *reading {
            Reading::Diameter { centre, at, .. } | Reading::Radius { centre, at, .. } => (Some(centre), Some(at), at),
            Reading::Distance { from, to, .. } => (Some(from), Some(to), [0, 1, 2].map(|k| (from[k] + to[k]) / 2.0)),
            Reading::Angle { at, .. } | Reading::ArcLength { at, .. } => (None, None, at),
        };
        if let (Some(a), Some(b)) = (from.and_then(to_screen), to.and_then(to_screen)) {
            painter.line_segment([a, b], stroke);
            painter.circle_filled(a, 2.5, INK);
            painter.circle_filled(b, 2.5, INK);
        }
        if let Some(at) = to_screen(label_at) {
            // Beside the point, one under another, until dragged away: then a leader with an
            // arrow points back at what it measures.
            let moved = m.moved.get(k).copied().flatten();
            let offset = moved.unwrap_or(vec2(8.0, -8.0 + 24.0 * k as f32));
            let card = label(painter, r, at + offset, &text(reading, units), moved.map(|_| at));
            hits.push((k, card));
        }
    }
    hits
}

/// A reading's value in a small card at `at`, kept inside the view `area` (moved in from an
/// edge it would run past), with a leader from the card to `points_to` when it has been moved
/// away. The card.
fn label(painter: &egui::Painter, area: Rect, at: Pos2, text: &str, points_to: Option<Pos2>) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), crate::theme::semibold(12.0), Color32::WHITE);
    let size = galley.size();
    let inner = area.shrink(4.0);
    let x = at.x.min(inner.right() - size.x).max(inner.left());
    let y = at.y.min(inner.bottom() - size.y).max(inner.top());
    let at = pos2(x, y);
    let card = Rect::from_min_size(at, size).expand(4.0);
    if let Some(tip) = points_to {
        // From the card's nearest edge to the point, an arrowhead at the point.
        let from = pos2(tip.x.clamp(card.left(), card.right()), tip.y.clamp(card.top(), card.bottom()));
        let d = tip - from;
        if d.length() > 10.0 {
            let dir = d.normalized();
            painter.line_segment([from, tip - dir * 6.0], Stroke::new(1.5, INK));
            let back = tip - dir * 10.0;
            let side = vec2(-dir.y, dir.x) * 4.0;
            painter.add(egui::Shape::convex_polygon(vec![tip, back + side, back - side], INK, Stroke::NONE));
        }
    }
    painter.rect_filled(card, CornerRadius::same(3), INK);
    painter.galley(at, galley, Color32::WHITE);
    card
}

/// The Measure and Dimension switches, what each shown model's measurement reads (with Keep,
/// onto the dimension layer being worked on), and that layer's dimensions, under the section
/// view.
pub(crate) fn measure_controls(ui: &mut egui::Ui, t: &crate::theme::Tokens, info: &pdfcraft_render::DocInfo, view: &mut super::View3d) {
    if !view.models.values().any(|m| matches!(m.load, super::Load::Ready(_))) {
        return;
    }
    let (was, was_dimensioning) = (view.measuring, view.dimensioning);
    ui.horizontal(|ui| {
        ui.add(crate::icons::image("ruler", 16.0, t.icon));
        ui.add(crate::widgets::checkbox(&mut view.measuring, egui::RichText::new(tl!("Measure")).font(crate::theme::medium(13.0))));
        ui.add_space(12.0);
        ui.add(crate::widgets::checkbox(&mut view.dimensioning, egui::RichText::new(tl!("Dimension")).font(crate::theme::medium(13.0))))
            .on_hover_text(tl!("Keep measurements on the selected part as dimensions, saved in the PDF"));
    });
    if view.dimensioning && !was_dimensioning {
        view.measuring = true;
        for m in view.models.values_mut() {
            // Follow the part selected now, again.
            m.dims_node = None;
        }
    }
    if !view.measuring {
        view.dimensioning = false;
    }
    if view.measuring && !was {
        // A fresh start: what was measured before is no longer shown.
        for m in view.models.values_mut() {
            m.measure.clear();
        }
    }
    if view.measuring {
        let mut any = false;
        let mut edit = None;
        for model in &info.annots_3d {
            let Some(m) = view.models.get_mut(&(model.page, model.index)) else { continue };
            let super::Load::Ready(scene) = &m.load else { continue };
            let scene = std::sync::Arc::clone(scene);
            let units = Units::of(&scene);
            let frame = m.isolated_frame(&scene).map(|(_, f)| f);
            let keeping = view.dimensioning && m.active.is_some();
            if view.dimensioning && m.active.is_none() {
                any = true;
                ui.label(egui::RichText::new(tl!("Click a part, in the picture or the tree, to dimension it.")).color(t.text_muted));
            }
            if !m.measure.picks.is_empty() {
                any = true;
                let mut keep = None;
                for (k, r) in m.measure.readings.iter().enumerate() {
                    ui.label(egui::RichText::new(kind(r)).color(t.text_muted).font(crate::theme::regular(12.0)));
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(text(r, units)).font(crate::theme::semibold(14.0)));
                        if keeping && ui.small_button(tl!("Keep")).on_hover_text(tl!("Keep it as a dimension on this part")).clicked() {
                            keep = Some(k);
                        }
                    });
                }
                if let [only] = m.measure.picks.as_slice()
                    && m.measure.readings.is_empty()
                {
                    // What the face picked is, when it has no size of its own to read.
                    let what = match only.fit.map(|f| f.shape) {
                        Some(pdfcraft_prc_render::shapes::Shape::Plane { .. }) => tl!("A flat face."),
                        _ => tl!("No simple shape here: a second click measures the distance between the points."),
                    };
                    ui.label(egui::RichText::new(what).font(crate::theme::medium(13.0)));
                    ui.label(egui::RichText::new(tl!("Click a second face to measure between them.")).color(t.text_muted));
                }
                if ui.button(if keeping { tl!("Drop") } else { tl!("Clear") }).clicked() {
                    m.measure.clear();
                }
                if let (Some(k), Some(frame), Some(camera), Some(active)) = (keep, frame, m.camera, m.active)
                    && let Some(r) = m.measure.readings.get(k)
                {
                    let dim = super::dims::dim_of(r, &frame, &camera, units);
                    edit = Some(m.layers_edit(model.page, model.index, |l| {
                        if let Some(layer) = l.get_mut(active) {
                            layer.dims.push(dim);
                        }
                    }));
                    m.dim_picked = m.layers.get(active).and_then(|l| l.dims.len().checked_sub(1));
                    m.measure.clear();
                }
            }
            if keeping {
                any = true;
                if let Some(e) = layer_controls(ui, t, m, &scene, (model.page, model.index), units) {
                    edit = Some(e);
                }
            }
        }
        if edit.is_some() {
            view.edit = edit;
        }
        if !any {
            ui.label(egui::RichText::new(tl!("Click a face of the model to measure it.")).color(t.text_muted));
        }
    }
    ui.add_space(4.0);
    ui.separator();
}

/// The dimension layer being worked on: its dimensions (one picked to set its tolerance or
/// delete it) and its saved view. The edit made.
fn layer_controls(
    ui: &mut egui::Ui,
    t: &crate::theme::Tokens,
    m: &mut super::Model,
    scene: &super::Scene,
    (page, index): (usize, usize),
    units: Units,
) -> Option<pdfcraft_engine::Edit> {
    let active = m.active?;
    let layer = m.layers.get(active)?;
    let mut edit = None;
    let copies = super::dims::copies(scene, &layer.part);
    ui.add_space(6.0);
    let title = crate::i18n::fmt(tl!("Dimensions on {part}"), &[("part", &layer.part)]);
    ui.label(egui::RichText::new(title).font(crate::theme::semibold(13.0)));
    if copies > 1 {
        let shared = crate::i18n::fmt(tl!("Shared by all {n} copies"), &[("n", &copies.to_string())]);
        ui.label(egui::RichText::new(shared).color(t.text_muted).font(crate::theme::regular(12.0)));
    }
    if layer.dims.is_empty() {
        ui.label(egui::RichText::new(tl!("Measure, then Keep, to add dimensions.")).color(t.text_muted));
    }
    let mut delete = None;
    let texts: Vec<String> = layer.dims.iter().map(|d| super::dims::dim_text(d, units)).collect();
    for (k, text) in texts.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.selectable_label(m.dim_picked == Some(k), text).clicked() {
                m.dim_picked = if m.dim_picked == Some(k) { None } else { Some(k) };
            }
            if crate::icons::button(ui, "x", 20.0, false, tl!("Delete this dimension")).clicked() {
                delete = Some(k);
            }
        });
    }
    if let Some(k) = delete {
        m.dim_picked = None;
        return Some(m.layers_edit(page, index, |l| {
            if let Some(layer) = l.get_mut(active)
                && k < layer.dims.len()
            {
                layer.dims.remove(k);
            }
        }));
    }
    // The picked dimension's tolerance and decimal places, kept when a change is finished.
    if let Some(d) = m.dim_picked.and_then(|k| m.layers.get_mut(active)?.dims.get_mut(k)) {
        let mut done = false;
        let step = 10f64.powi(-i32::from(d.places));
        let suffix = if d.kind == pdfcraft_engine::DimKind3d::Angle { "°" } else { "" };
        ui.horizontal(|ui| {
            ui.label(tl!("Tolerance"));
            ui.label("+");
            let plus = ui.add(egui::DragValue::new(&mut d.plus).range(0.0..=1e6).speed(step).max_decimals(6).suffix(suffix));
            ui.label("−");
            let minus = ui.add(egui::DragValue::new(&mut d.minus).range(0.0..=1e6).speed(step).max_decimals(6).suffix(suffix));
            done |= [plus, minus].iter().any(|r| (r.changed() && !r.dragged()) || r.drag_stopped());
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Decimal places"));
            let places = ui.add(egui::DragValue::new(&mut d.places).range(0..=6).speed(0.05));
            done |= (places.changed() && !places.dragged()) || places.drag_stopped();
        });
        if done {
            edit = Some(pdfcraft_engine::Edit::SetModel3dLayers { page, index, layers: m.layers.clone() });
        }
    }
    // The layer's view: the camera it opens on.
    let frame = m.isolated_frame(scene).map(|(_, f)| f);
    let saved = m.layers.get(active).and_then(|l| l.view);
    let (mut go, mut save) = (false, false);
    ui.horizontal(|ui| {
        go = saved.is_some() && ui.button(tl!("Go to saved view")).clicked();
        save = ui.button(tl!("Save this view")).on_hover_text(tl!("The layer opens on the view as it is now")).clicked();
    });
    if let (true, Some(v), Some(f)) = (go, saved, frame) {
        m.camera = Some(f.camera(&v));
    }
    if let (true, Some(c), Some(f)) = (save, m.camera, frame) {
        let v = f.save_view(&c);
        edit = Some(m.layers_edit(page, index, |l| {
            if let Some(layer) = l.get_mut(active) {
                layer.view = Some(v);
            }
        }));
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_in_the_models_unit_with_the_other_beside() {
        let inch = Units { mm_per_unit: 25.4, stated: true };
        assert_eq!(inch.length(0.75), "0.750 in (19.05 mm)");
        let mm = Units { mm_per_unit: 1.0, stated: true };
        assert_eq!(mm.length(19.05), "19.05 mm (0.750 in)");
        let assumed = Units { mm_per_unit: 1.0, stated: false };
        assert_eq!(assumed.length(10.0), "10.00 mm? (0.394 in)");
        let r = Reading::Distance { value: 2.1, from: [0.0; 3], to: [0.0; 3], exact: false };
        assert_eq!(text(&r, inch), "≈ 2.100 in (53.34 mm)");
        assert_eq!(text(&Reading::Diameter { value: 0.5, centre: [0.0; 3], at: [0.0; 3] }, inch), "Ø 0.500 in (12.70 mm)");
    }
}
