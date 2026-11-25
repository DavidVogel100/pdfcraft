//! The Model Tree panel: a 3D model's assembly as a CAD feature tree lists it (assemblies and
//! parts by name, nested, with their instance numbers as `Name<1>`), where a part can be picked,
//! hidden, shown or isolated, and the section view that cuts the model by one of its three planes.

use std::collections::HashSet;

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use pdfcraft_prc_render::{Look, Section};
use pdfcraft_render::DocInfo;

use super::scene::Scene;
use super::{Load, View3d};
use crate::icons;
use crate::theme::{self, Tokens};

/// Rows nest at most this deep in the panel; real assemblies are far shallower.
const MAX_DEPTH: usize = 48;
const ROW: f32 = 22.0;
const INDENT: f32 = 14.0;

/// What the user has done to one model's parts.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parts {
    /// Nodes the user hid (with everything under them).
    pub(crate) hidden: HashSet<usize>,
    pub(crate) selected: Option<usize>,
    /// Only this node (and what is under it) is shown.
    pub(crate) isolated: Option<usize>,
    pub(crate) expanded: HashSet<usize>,
    /// A row to bring into view (a part picked on the page).
    pub(crate) scroll_to: Option<usize>,
    /// Bumped on every change, so the look is rebuilt and the model redrawn.
    pub(crate) rev: u64,
    /// Parts moved by hand while taken apart: how far, in the world.
    pub(crate) moved: std::collections::HashMap<usize, [f64; 3]>,
    /// Parts highlighted with the selection: those a mate picked in the Mates list joins.
    pub(crate) also: HashSet<usize>,
}

impl Parts {
    pub(crate) fn changed(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Selects `node` (or nothing), opening the rows above it so it can be seen.
    pub(crate) fn select(&mut self, scene: &Scene, node: Option<usize>) {
        self.selected = node;
        if let Some(n) = node {
            let mut up = scene.nodes.get(n).and_then(|x| x.parent);
            while let Some(p) = up {
                self.expanded.insert(p);
                up = scene.nodes.get(p).and_then(|x| x.parent);
            }
            self.scroll_to = Some(n);
        }
        self.changed();
    }
}

/// The CAD plane a section cuts along, named as SOLIDWORKS names them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Plane {
    /// The XY plane: cuts along Z.
    #[default]
    Front,
    /// The XZ plane: cuts along Y.
    Top,
    /// The YZ plane: cuts along X.
    Right,
}

impl Plane {
    fn axis(self) -> usize {
        match self {
            Self::Right => 0,
            Self::Top => 1,
            Self::Front => 2,
        }
    }
}

/// Taking the model apart, shared by the document's models.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) struct ExplodeState {
    pub(crate) on: bool,
    /// How far apart, 0 (together) to 1 (each part twice as far again from the middle as it is).
    pub(crate) amount: f32,
    /// A drag on a part moves it, rather than turning the model.
    pub(crate) move_parts: bool,
}

impl ExplodeState {
    /// Whether the parts are not where the model places them.
    pub(crate) fn apart(&self) -> bool {
        self.on
    }
}

/// How far each part is from its place: out from the middle of what is shown, by `amount`
/// (at 1, twice as far again as it is from the middle), plus any move by hand.
pub(crate) fn part_offsets(scene: &Scene, parts: &Parts, explode: &ExplodeState) -> Option<Vec<[f64; 3]>> {
    if !explode.on {
        return None;
    }
    // The middle of what is shown: the part isolated, else the whole model.
    let root_centre = parts.isolated.and_then(|i| scene.centres.get(i).copied().flatten()).or_else(|| scene.bounds.map(|b| b.centre()))?;
    let k = 2.0 * f64::from(explode.amount.clamp(0.0, 1.0));
    Some(
        scene
            .mesh_nodes
            .iter()
            .map(|&node| {
                let Some(node) = node else { return [0.0; 3] };
                let centre = scene.centres.get(node).copied().flatten().unwrap_or(root_centre);
                let by_hand = parts.moved.get(&node).copied().unwrap_or([0.0; 3]);
                [0, 1, 2].map(|a| (centre[a] - root_centre[a]) * k + by_hand[a])
            })
            .collect(),
    )
}

/// The explode controls, above the tree: on or off, how far apart, moving parts by hand.
pub(crate) fn explode_controls(ui: &mut egui::Ui, t: &Tokens, view: &mut View3d) {
    if !view.models.values().any(|m| matches!(m.load, Load::Ready(_))) {
        return;
    }
    let before = view.explode;
    let e = &mut view.explode;
    ui.horizontal(|ui| {
        ui.add(icons::image("move", 16.0, t.icon));
        ui.add(crate::widgets::checkbox(&mut e.on, egui::RichText::new(tl!("Explode")).font(theme::medium(13.0))))
            .on_hover_text(tl!("Take the assembly apart to see every part"));
    });
    if e.on {
        ui.label(egui::RichText::new(tl!("How far apart")).color(t.text_muted).font(theme::regular(12.0)));
        cut_slider(ui, t, &mut e.amount).on_hover_text(tl!("Drag to spread the parts out or bring them together"));
        ui.horizontal(|ui| {
            ui.add(crate::widgets::checkbox(&mut e.move_parts, tl!("Drag parts to move them")));
            if ui.button(tl!("Put back")).on_hover_text(tl!("Every part back where the model places it")).clicked() {
                for m in view.models.values_mut() {
                    m.parts.moved.clear();
                    m.parts.changed();
                }
            }
        });
    }
    ui.add_space(4.0);
    ui.separator();
    if view.explode != before {
        if view.explode.on && !before.on && view.explode.amount == 0.0 {
            view.explode.amount = 0.25;
        }
        if !view.explode.on {
            view.explode.move_parts = false;
        }
        for m in view.models.values_mut() {
            m.parts.changed();
        }
    }
}

/// Print part…, Save as STL… and Save as STEP…, above the tree so they stay in sight however
/// far it is scrolled: for the model on `page`, else the document's first.
pub(crate) fn file_controls(ui: &mut egui::Ui, t: &Tokens, page: usize, view: &mut View3d) {
    let Some(key) = view.ready_model(page) else { return };
    let isolated = view.models.get(&key).and_then(|m| m.parts.isolated).is_some();
    let (mut print, mut stl, mut step) = (false, false, false);
    ui.horizontal(|ui| {
        ui.add(icons::image("file-down", 16.0, t.icon));
        print = isolated && ui.button(tl!("Print part…")).on_hover_text(tl!("A sheet of this part with its dimensions and notes, to print or save")).clicked();
        stl = ui.button(tl!("Save as STL…")).on_hover_text(tl!("Save what is shown as an STL file, for 3D printing (also File ▸ Export)")).clicked();
        step = ui
            .button(tl!("Save as STEP…"))
            .on_hover_text(tl!("Save what is shown as a STEP file of facets, to open in a CAD program (also File ▸ Export)"))
            .clicked();
    });
    ui.add_space(4.0);
    ui.separator();
    if print {
        view.sheet_dialog = Some(key);
        view.sheet_error = None;
    }
    if stl || step {
        view.export = view.export_model(page, step);
    }
}

/// The section view, shared by the document's models.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SectionState {
    pub(crate) on: bool,
    pub(crate) plane: Plane,
    /// Where the plane lies across the model's bounds along its axis, 0 to 1.
    pub(crate) at: f32,
    /// Cut away the other side.
    pub(crate) flip: bool,
}

impl Default for SectionState {
    fn default() -> Self {
        Self { on: false, plane: Plane::Front, at: 0.5, flip: false }
    }
}

impl SectionState {
    /// The plane in model coordinates for a model within `lo`..`hi`.
    pub(crate) fn section(&self, lo: [f64; 3], hi: [f64; 3]) -> Option<Section> {
        if !self.on {
            return None;
        }
        let k = self.plane.axis();
        let (lo, hi) = (lo.get(k).copied().unwrap_or(0.0), hi.get(k).copied().unwrap_or(0.0));
        let at = lo + (hi - lo) * f64::from(self.at.clamp(0.0, 1.0));
        let mut normal = [0.0; 3];
        if let Some(n) = normal.get_mut(k) {
            *n = 1.0;
        }
        let (normal, offset) = if self.flip { (normal.map(|v: f64| -v), -at) } else { (normal, at) };
        Some(Section { normal, offset })
    }
}

/// How `scene` is drawn given what the user did to its parts and the section view.
pub(crate) fn look_of(scene: &Scene, parts: &Parts, section: Option<Section>, explode: &ExplodeState) -> Look {
    // Nodes come before the nodes under them, so one pass in order settles each from its parent.
    let n = scene.nodes.len();
    let (mut hidden, mut inside, mut picked) = (vec![false; n], vec![parts.isolated.is_none(); n], vec![false; n]);
    for (i, node) in scene.nodes.iter().enumerate() {
        let parent = node.parent.filter(|&p| p < i);
        hidden[i] = parts.hidden.contains(&i) || parent.is_some_and(|p| hidden[p]);
        inside[i] = parts.isolated.is_none() || parts.isolated == Some(i) || parent.is_some_and(|p| inside[p]);
        picked[i] = parts.selected == Some(i) || parts.also.contains(&i) || parent.is_some_and(|p| picked[p]);
    }
    let of = |v: &[bool], node: Option<usize>, none: bool| node.and_then(|k| v.get(k).copied()).unwrap_or(none);
    Look {
        hidden: scene.mesh_nodes.iter().map(|&node| of(&hidden, node, false) || !of(&inside, node, parts.isolated.is_none())).collect(),
        highlighted: scene.mesh_nodes.iter().map(|&node| of(&picked, node, false)).collect(),
        section,
        outlines: Some(std::sync::Arc::clone(&scene.outlines)),
        offsets: part_offsets(scene, parts, explode).map(std::sync::Arc::from),
    }
}

/// A node's name as SOLIDWORKS shows it: its instance number `-1` written `<1>`.
pub(crate) fn display_name(name: &str) -> String {
    match name.rsplit_once('-') {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => format!("{base}<{n}>"),
        _ => name.to_owned(),
    }
}

/// The rows under `node`: its children, looking through a configuration (SOLIDWORKS writes the
/// active configuration, "Default", as the one occurrence directly under an assembly).
pub(crate) fn shown_children(scene: &Scene, node: usize) -> &[usize] {
    let Some(n) = scene.nodes.get(node) else { return &[] };
    if let [only] = n.children.as_slice()
        && let Some(c) = scene.nodes.get(*only)
        && c.meshes.is_empty()
        && !c.children.is_empty()
        && display_name(&c.name) == c.name
    {
        return &c.children;
    }
    &n.children
}

/// Whether anything is drawn under `node`.
fn has_meshes(scene: &Scene, node: usize) -> bool {
    scene.subtree(node).into_iter().any(|k| scene.nodes.get(k).is_some_and(|n| !n.meshes.is_empty()))
}

/// The section view's controls, above the tree.
pub(crate) fn section_controls(ui: &mut egui::Ui, t: &Tokens, view: &mut View3d) {
    if !view.models.values().any(|m| matches!(m.load, Load::Ready(_))) {
        return;
    }
    let before = view.section;
    let s = &mut view.section;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add(icons::image("scissors", 16.0, t.icon));
        ui.add(crate::widgets::checkbox(&mut s.on, egui::RichText::new(tl!("Section view")).font(theme::medium(13.0))));
    });
    if s.on {
        ui.horizontal(|ui| {
            for (plane, label) in [(Plane::Front, "Front"), (Plane::Top, "Top"), (Plane::Right, "Right")] {
                ui.selectable_value(&mut s.plane, plane, tl!(label));
            }
            ui.add(crate::widgets::checkbox(&mut s.flip, tl!("Flip")));
        });
        ui.label(egui::RichText::new(tl!("Cut position")).color(t.text_muted).font(theme::regular(12.0)));
        cut_slider(ui, t, &mut s.at).on_hover_text(tl!("Drag to move the cut through the model"));
    }
    ui.add_space(4.0);
    ui.separator();
    if view.section != before {
        for m in view.models.values_mut() {
            m.parts.changed();
        }
    }
}

/// A slider for where the cut lies, 0 to 1: a track, filled up to the knob.
fn cut_slider(ui: &mut egui::Ui, t: &Tokens, at: &mut f32) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width().max(80.0), 22.0), Sense::click_and_drag());
    let (left, right) = (rect.left() + 8.0, rect.right() - 8.0);
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let v = ((p.x - left) / (right - left).max(1.0)).clamp(0.0, 1.0);
        if v != *at {
            *at = v;
            resp.mark_changed();
        }
    }
    let x = left + (right - left) * at.clamp(0.0, 1.0);
    let y = rect.center().y;
    let painter = ui.painter();
    painter.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(4.0, t.border));
    painter.line_segment([pos2(left, y), pos2(x, y)], Stroke::new(4.0, t.accent));
    let hot = resp.hovered() || resp.dragged();
    painter.circle(pos2(x, y), if hot { 8.0 } else { 7.0 }, t.card, Stroke::new(2.0, t.accent));
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(*at), tl!("Cut position")));
    theme::hand(resp)
}

/// The Model Tree: each shown 3D model's assembly.
pub(crate) fn tree_panel(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, view: &mut View3d) {
    let several = info.annots_3d.len() > 1;
    let mut any = false;
    for model in &info.annots_3d {
        let Some(m) = view.models.get_mut(&(model.page, model.index)) else { continue };
        let scene = match &m.load {
            Load::Ready(scene) => std::sync::Arc::clone(scene),
            Load::Pending(_) => {
                ui.label(egui::RichText::new(tl!("Loading 3D model…")).color(t.text_muted));
                any = true;
                continue;
            }
            Load::Failed(_) => continue,
        };
        any = true;
        if several {
            let page = (model.page + 1).to_string();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("3D model on page {n}"), &[("n", &page)])).font(theme::semibold(13.0)));
        }
        let parts = &mut m.parts;
        if scene.nodes.is_empty() {
            ui.label(egui::RichText::new(tl!("This model has no named parts.")).color(t.text_muted));
            continue;
        }
        if parts.isolated.is_some() || !parts.hidden.is_empty() {
            ui.horizontal(|ui| {
                if let Some(iso) = parts.isolated.and_then(|i| scene.nodes.get(i)) {
                    let text = crate::i18n::fmt(tl!("Showing only {name}"), &[("name", &display_name(&iso.name))]);
                    ui.label(egui::RichText::new(text).color(t.accent_text).font(theme::medium(12.0)));
                }
                if ui.button(tl!("Show all")).clicked() {
                    parts.isolated = None;
                    parts.hidden.clear();
                    parts.changed();
                }
            });
        }
        let key = (model.page, model.index);
        if view.sheet_dialog == Some(key) {
            match sheet_dialog(ui.ctx(), &mut view.sheet_choice, view.sheet_error.as_deref()) {
                Some(true) => {
                    let made = parts.isolated.zip(m.camera).ok_or_else(|| tl!("Isolate a part first.").to_owned()).and_then(|(node, camera)| {
                        let part = super::dims::part_name(&scene, node);
                        let aspect = m.area.or(m.screen).map_or(1.0, |r| f64::from(r.width() / r.height().max(1.0)));
                        let subject = super::sheet::Subject {
                            scene: &scene,
                            node,
                            camera,
                            aspect,
                            look: &m.look,
                            layers: super::sheet::shown_layers(&m.layers, &part, &m.hidden_layers),
                            units: super::measure::Units::of(&scene),
                            file: &view.file_name,
                            date: super::sheet::today(),
                        };
                        super::sheet::make(&subject, view.sheet_choice)
                    });
                    match made {
                        Ok(s) => {
                            view.sheet = Some(s);
                            view.sheet_dialog = None;
                        }
                        Err(e) => view.sheet_error = Some(e),
                    }
                }
                Some(false) => view.sheet_dialog = None,
                None => {}
            }
        }
        let parts = &mut m.parts;
        let mut layers = TreeLayers { layers: &m.layers, hidden: &mut m.hidden_layers, open: None };
        for root in scene.roots().collect::<Vec<_>>() {
            parts.expanded.insert(root);
            row(ui, t, &scene, parts, &mut layers, root, 0, false);
        }
        mates_list(ui, t, &scene, &mut m.parts, &mut m.mates);
        let parts = &mut m.parts;
        // A layer opened: its part isolated and turned to the layer's view.
        if let Some((node, k)) = layers.open {
            parts.selected = Some(node);
            parts.isolated = Some(node);
            let under = scene.subtree(node);
            parts.hidden.retain(|h| !under.contains(h));
            parts.changed();
            m.hidden_layers.remove(&k);
            m.dims_node = Some(node);
            let kind = m.layers.get(k).map(|l| l.kind);
            if (view.dimensioning && kind == Some(pdfcraft_engine::LayerKind3d::Dimensions))
                || (view.noting && kind == Some(pdfcraft_engine::LayerKind3d::Notes))
            {
                m.active = Some(k);
            }
            if let Some(v) = m.layers.get(k).and_then(|l| l.view) {
                m.camera = Some(super::dims::frame_of(&scene, node).camera(&v));
            }
        }
        ui.add_space(8.0);
    }
    if !any {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("compass", 36.0, t.text_faint));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tl!("Scroll to a 3D model to see its parts.")).color(t.text_muted));
        });
    }
}

/// Print part's choices: the paper, its way round and the title block. `Some(true)` to make
/// the sheet, `Some(false)` to cancel, `None` while open.
fn sheet_dialog(ctx: &egui::Context, choice: &mut super::sheet::SheetChoice, error: Option<&str>) -> Option<bool> {
    let mut answer = None;
    let modal = crate::widgets::show_modal(ctx, "view3d-print-part", |ui| {
        ui.set_width(380.0);
        ui.label(egui::RichText::new(tl!("Print part")).font(theme::semibold(15.0)));
        ui.add_space(4.0);
        ui.label(tl!("The part as it is shown, seen square-on, with its dimensions and notes. It opens as a new PDF to print or save."));
        ui.add_space(8.0);
        egui::Grid::new("view3d-sheet-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Paper"));
            craft_ui::ComboBox::from_id_salt("view3d-sheet-paper").selected_text(tl!(choice.paper.name())).show_ui(ui, |ui| {
                for p in super::sheet::Paper::ALL {
                    ui.selectable_value(&mut choice.paper, p, tl!(p.name()));
                }
            });
            ui.end_row();
            ui.label(tl!("Orientation"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut choice.landscape, true, tl!("Landscape"));
                ui.selectable_value(&mut choice.landscape, false, tl!("Portrait"));
            });
            ui.end_row();
        });
        ui.add(crate::widgets::checkbox(&mut choice.title_block, tl!("Title block")));
        if let Some(e) = error {
            ui.label(egui::RichText::new(e).color(Color32::from_rgb(0xC0, 0x2B, 0x2B)));
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::pill_button(ui, tl!("Make sheet"), true).clicked() {
                answer = Some(true);
            }
            if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    answer
}

/// The model's dimension layers as the tree lists them under their parts, and the one opened.
struct TreeLayers<'a> {
    layers: &'a [pdfcraft_engine::Layer3d],
    hidden: &'a mut HashSet<usize>,
    /// The node and layer clicked.
    open: Option<(usize, usize)>,
}

/// One node's row and, when it is open, the rows under it: its part's layers, then its children.
#[allow(clippy::too_many_arguments)]
fn row(ui: &mut egui::Ui, t: &Tokens, scene: &Scene, parts: &mut Parts, layers: &mut TreeLayers<'_>, node: usize, depth: usize, parent_hidden: bool) {
    let Some(n) = scene.nodes.get(node) else { return };
    let kids = shown_children(scene, node);
    let part = super::dims::part_name(scene, node);
    let own: Vec<usize> = (0..layers.layers.len()).filter(|&k| layers.layers.get(k).is_some_and(|l| l.part == part)).collect();
    let open = parts.expanded.contains(&node);
    let hidden = parent_hidden || parts.hidden.contains(&node);
    let outside = parts.isolated.is_some_and(|iso| iso != node && !scene.subtree(iso).contains(&node) && !scene.subtree(node).contains(&iso));
    let empty = !has_meshes(scene, node);
    let selected = parts.selected == Some(node);

    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let name = display_name(&n.name);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, &name));
    if parts.scroll_to == Some(node) {
        resp.scroll_to_me(Some(egui::Align::Center));
        parts.scroll_to = None;
    }
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 4, selected);
    let painter = ui.painter_at(rect);
    let x = rect.left() + 2.0 + depth.min(MAX_DEPTH) as f32 * INDENT;
    // The expander.
    let arrow = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(16.0, 16.0));
    let mut toggled = false;
    if !kids.is_empty() || !own.is_empty() {
        let arrow_resp = ui.interact(arrow, resp.id.with("expand"), Sense::click());
        icons::paint(ui, arrow, if open { "chevron-down" } else { "chevron-right" }, 13.0, t.text_muted);
        if arrow_resp.clicked() {
            if open {
                parts.expanded.remove(&node);
            } else {
                parts.expanded.insert(node);
            }
            toggled = true;
        }
    }
    // The icon: an assembly or a part, outlined when hidden.
    let icon = Rect::from_min_size(pos2(x + 18.0, rect.top() + 3.0), vec2(16.0, 16.0));
    paint_icon(&painter, icon, !kids.is_empty(), hidden || outside || empty);
    let faded = hidden || outside || empty;
    let colour = if faded { t.text_faint } else { t.text };
    // The eye, at the end of a hovered, selected or hidden row; the name stops short of it.
    let eye = Rect::from_min_size(pos2(rect.right() - 22.0, rect.top() + 3.0), vec2(16.0, 16.0));
    let text_area = Rect::from_min_max(rect.min, pos2(eye.left() - 4.0, rect.bottom()));
    ui.painter_at(text_area).text(pos2(icon.right() + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, &name, theme::regular(13.0), colour);
    if !own.is_empty() {
        // The part has dimensions: a ruler before the eye.
        let ruler = Rect::from_min_size(pos2(eye.left() - 20.0, rect.top() + 3.0), vec2(16.0, 16.0));
        icons::paint(ui, ruler, "ruler", 13.0, t.accent_text);
    }
    let eye_resp = ui.interact(eye, resp.id.with("eye"), Sense::click());
    if (resp.hovered() || eye_resp.hovered() || selected || parts.hidden.contains(&node)) && !empty {
        let tint = if eye_resp.hovered() { t.text } else { t.text_muted };
        icons::paint(ui, eye, if parts.hidden.contains(&node) { "eye-off" } else { "eye" }, 14.0, tint);
    }
    let eye_resp = eye_resp.on_hover_text(if parts.hidden.contains(&node) { tl!("Show") } else { tl!("Hide") });
    if eye_resp.clicked() && !empty {
        if !parts.hidden.remove(&node) {
            parts.hidden.insert(node);
        }
        parts.changed();
    } else if resp.clicked() && !toggled {
        parts.select(scene, if selected { None } else { Some(node) });
    }
    if resp.double_clicked() && !kids.is_empty() {
        if open {
            parts.expanded.remove(&node);
        } else {
            parts.expanded.insert(node);
        }
    }
    let resp = resp.on_hover_text(&name);
    resp.context_menu(|ui| {
        let is_hidden = parts.hidden.contains(&node);
        if ui.button(if is_hidden { tl!("Show") } else { tl!("Hide") }).clicked() {
            if is_hidden {
                parts.hidden.remove(&node);
            } else {
                parts.hidden.insert(node);
            }
            parts.changed();
            ui.close();
        }
        if parts.isolated == Some(node) {
            if ui.button(tl!("Exit isolate")).clicked() {
                parts.isolated = None;
                parts.changed();
                ui.close();
            }
        } else if ui.button(tl!("Isolate")).clicked() {
            parts.isolated = Some(node);
            parts.hidden.retain(|&h| !scene.subtree(node).contains(&h));
            parts.changed();
            ui.close();
        }
        if ui.button(tl!("Show all")).clicked() {
            parts.isolated = None;
            parts.hidden.clear();
            parts.changed();
            ui.close();
        }
    });
    if open && depth < MAX_DEPTH {
        for &k in &own {
            layer_row(ui, t, scene, layers, node, k, depth + 1);
        }
        let hidden_here = parent_hidden || parts.hidden.contains(&node);
        for &k in kids {
            row(ui, t, scene, parts, layers, k, depth + 1, hidden_here);
        }
    }
}

/// The Mates folder under the tree, as SOLIDWORKS lists it: with a part selected, the mates that
/// hold it; else all of them. A click on a mate highlights the parts it joins and draws its axes
/// and planes on the model; another click lets it go.
fn mates_list(ui: &mut egui::Ui, t: &Tokens, scene: &Scene, parts: &mut Parts, state: &mut super::mates::MateState) {
    use super::mates;
    if let Some(e) = &state.error {
        ui.add_space(6.0);
        let text = crate::i18n::fmt(tl!("The mates attachment can't be read: {e}"), &[("e", e)]);
        ui.label(egui::RichText::new(text).color(t.text_muted));
        return;
    }
    let Some(all) = state.mates.clone() else { return };
    let units = super::measure::Units::of(scene);
    let (list, title) = match parts.selected.and_then(|n| scene.nodes.get(n).map(|x| (n, x))) {
        Some((n, node)) => (all.of_node(scene, n), crate::i18n::fmt(tl!("Mates of {name}"), &[("name", &display_name(&node.name))])),
        None => ((0..all.mates.len()).collect::<Vec<_>>(), tl!("Mates").to_owned()),
    };
    // A mate picked that the list no longer shows lets go of its parts.
    if state.picked.is_some_and(|k| !list.contains(&k)) {
        state.picked = None;
        parts.also.clear();
        parts.changed();
    }
    ui.add_space(6.0);
    ui.separator();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let header = format!("{title} ({})", list.len());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TreeItem, true, &header));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 4, false);
    icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 2.0, rect.top() + 3.0), vec2(16.0, 16.0)), if state.open { "chevron-down" } else { "chevron-right" }, 13.0, t.text_muted);
    icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 20.0, rect.top() + 3.0), vec2(16.0, 16.0)), "link", 13.0, t.accent_text);
    ui.painter_at(rect).text(pos2(rect.left() + 42.0, rect.center().y), egui::Align2::LEFT_CENTER, &header, theme::medium(13.0), t.text);
    let resp = resp.on_hover_text(crate::i18n::fmt(tl!("From {file}, written by {source}"), &[("file", &all.file), ("source", &all.source)]));
    if resp.clicked() {
        state.open = !state.open;
    }
    if !state.open {
        return;
    }
    if list.is_empty() {
        ui.label(egui::RichText::new(tl!("No mates hold this part.")).color(t.text_muted));
    }
    for k in list {
        let Some(mate) = all.mates.get(k) else { continue };
        let joined: Vec<String> = mate.entities.iter().filter_map(|e| e.component.as_deref()).map(|c| display_name(c.rsplit('/').next().unwrap_or(c))).collect();
        let value = mates::value_text(mate, units);
        let mut text = format!("{} ({})", mate.name, joined.join(", "));
        if !value.is_empty() {
            text.push_str(&format!("  {value}"));
        }
        let picked = state.picked == Some(k);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, picked, &text));
        let press = theme::Press::track(ui, &resp);
        press.wash(ui, rect, 4, picked);
        let colour = if mate.suppressed { t.text_faint } else { t.text };
        icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 20.0, rect.top() + 3.0), vec2(16.0, 16.0)), mates::icon(mate), 13.0, if mate.suppressed { t.text_faint } else { t.accent_text });
        ui.painter_at(rect).text(pos2(rect.left() + 42.0, rect.center().y), egui::Align2::LEFT_CENTER, &text, theme::regular(13.0), colour);
        let mut tip = format!("{}: {}", mates::type_name(mate), joined.join(" ↔ "));
        if !value.is_empty() {
            tip.push_str(&format!("\n{value}"));
        }
        if let Some(a) = mate.alignment.as_deref() {
            tip.push_str(&format!("\n{}", crate::i18n::fmt(tl!("Alignment: {a}"), &[("a", a)])));
        }
        if mate.suppressed {
            tip.push_str(&format!("\n{}", tl!("Suppressed")));
        }
        if resp.on_hover_text(tip).clicked() {
            if picked {
                state.picked = None;
                parts.also.clear();
            } else {
                state.picked = Some(k);
                parts.also = all.joined(k).into_iter().collect();
            }
            parts.changed();
        }
    }
    if !all.missing.is_empty() {
        let text = crate::i18n::fmt(tl!("{n} parts the mates name are not in this model"), &[("n", &all.missing.len().to_string())]);
        ui.label(egui::RichText::new(text).color(t.text_muted)).on_hover_text(all.missing.join("\n"));
    }
}

/// A layer's row under its part: a click opens it, the eye shows or hides it.
fn layer_row(ui: &mut egui::Ui, t: &Tokens, scene: &Scene, layers: &mut TreeLayers<'_>, node: usize, k: usize, depth: usize) {
    let Some(layer) = layers.layers.get(k) else { return };
    let copies = super::dims::copies(scene, &layer.part);
    let notes = layer.kind == pdfcraft_engine::LayerKind3d::Notes;
    let fallback = if notes { tl!("Notes") } else { tl!("Dimensions") };
    let mut name = if layer.name.is_empty() { fallback.to_owned() } else { layer.name.clone() };
    if copies > 1 {
        name.push_str(&crate::i18n::fmt(tl!(" (shared by {n} copies)"), &[("n", &copies.to_string())]));
    }
    let hidden = layers.hidden.contains(&k);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TreeItem, true, &name));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 4, false);
    let x = rect.left() + 2.0 + depth.min(MAX_DEPTH) as f32 * INDENT + 18.0;
    icons::paint(
        ui,
        Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(16.0, 16.0)),
        if notes { "sticky-note" } else { "ruler" },
        13.0,
        if hidden { t.text_faint } else { t.accent_text },
    );
    let eye = Rect::from_min_size(pos2(rect.right() - 22.0, rect.top() + 3.0), vec2(16.0, 16.0));
    let text_area = Rect::from_min_max(rect.min, pos2(eye.left() - 4.0, rect.bottom()));
    let colour = if hidden { t.text_faint } else { t.text };
    ui.painter_at(text_area).text(pos2(x + 22.0, rect.center().y), egui::Align2::LEFT_CENTER, &name, theme::regular(13.0), colour);
    let eye_resp = ui.interact(eye, resp.id.with("eye"), Sense::click());
    icons::paint(ui, eye, if hidden { "eye-off" } else { "eye" }, 14.0, if eye_resp.hovered() { t.text } else { t.text_muted });
    if eye_resp.on_hover_text(if hidden { tl!("Show") } else { tl!("Hide") }).clicked() {
        if !layers.hidden.remove(&k) {
            layers.hidden.insert(k);
        }
    } else if resp.on_hover_text(if notes { tl!("Show this part with its notes") } else { tl!("Show this part with its dimensions") }).clicked() {
        layers.open = Some((node, k));
    }
}

/// A small cube (a part) or two (an assembly), as CAD trees draw them; only outlined when hidden.
fn paint_icon(painter: &egui::Painter, r: Rect, assembly: bool, faded: bool) {
    let cube = |painter: &egui::Painter, c: Pos2, s: f32, top: Color32, left: Color32, right: Color32, line: Color32| {
        let (h, w) = (s * 0.5, s * 0.866);
        let up = pos2(c.x, c.y - s);
        let (tl, tr, mid, bl, br, bottom) =
            (pos2(c.x - w, c.y - h), pos2(c.x + w, c.y - h), c, pos2(c.x - w, c.y + h), pos2(c.x + w, c.y + h), pos2(c.x, c.y + s));
        let stroke = Stroke::new(1.0, line);
        painter.add(Shape::convex_polygon(vec![up, tr, mid, tl], top, stroke));
        painter.add(Shape::convex_polygon(vec![tl, mid, bottom, bl], left, stroke));
        painter.add(Shape::convex_polygon(vec![mid, tr, br, bottom], right, stroke));
    };
    let (top, left, right, line) = if faded {
        (Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::from_gray(150))
    } else {
        (
            Color32::from_rgb(0xFF, 0xE0, 0x7A),
            Color32::from_rgb(0xF2, 0xB8, 0x2E),
            Color32::from_rgb(0xC9, 0x8B, 0x10),
            Color32::from_rgb(0x7A, 0x55, 0x00),
        )
    };
    if assembly {
        let (blue_top, blue_left, blue_right) = if faded {
            (Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT)
        } else {
            (Color32::from_rgb(0x9C, 0xC4, 0xFF), Color32::from_rgb(0x4F, 0x8F, 0xF0), Color32::from_rgb(0x2B, 0x63, 0xC8))
        };
        cube(painter, pos2(r.left() + 5.5, r.top() + 10.0), 4.6, blue_top, blue_left, blue_right, line);
        cube(painter, pos2(r.left() + 10.5, r.top() + 7.0), 4.6, top, left, right, line);
    } else {
        cube(painter, r.center(), 6.5, top, left, right, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view3d_ui::scene::Node;

    /// Assembly "A" holding the configuration "Default", which holds "Sub-1" (with "Part-1" and
    /// "Part-2") and "Bolt-3"; one mesh per part.
    fn scene() -> Scene {
        let node = |name: &str, parent: Option<usize>, children: Vec<usize>, meshes: Vec<usize>| Node { name: name.into(), parent, children, meshes };
        Scene {
            nodes: vec![
                node("A", None, vec![1], vec![]),
                node("Default", Some(0), vec![2, 5], vec![]),
                node("Sub-1", Some(1), vec![3, 4], vec![]),
                node("Part-1", Some(2), vec![], vec![0]),
                node("Part-2", Some(2), vec![], vec![1]),
                node("Bolt-3", Some(1), vec![], vec![2]),
            ],
            mesh_nodes: vec![Some(3), Some(4), Some(5)],
            ..Scene::default()
        }
    }

    #[test]
    fn names_read_as_solidworks_writes_them_and_the_configuration_is_looked_through() {
        assert_eq!(display_name("Bottle Side Guides-2"), "Bottle Side Guides<2>");
        assert_eq!(display_name("SHCS 10-24 x ALL-12"), "SHCS 10-24 x ALL<12>");
        assert_eq!(display_name("Default"), "Default");
        assert_eq!(display_name("x-"), "x-");
        let s = scene();
        assert_eq!(shown_children(&s, 0), [2, 5]);
        assert_eq!(shown_children(&s, 2), [3, 4]);
    }

    #[test]
    fn hiding_isolating_and_selecting_reach_the_meshes_under_a_node() {
        let s = scene();
        let mut parts = Parts::default();
        assert_eq!(look_of(&s, &parts, None, &ExplodeState::default()).hidden, [false, false, false]);
        parts.hidden.insert(2);
        assert_eq!(look_of(&s, &parts, None, &ExplodeState::default()).hidden, [true, true, false]);
        parts.hidden.clear();
        parts.isolated = Some(4);
        assert_eq!(look_of(&s, &parts, None, &ExplodeState::default()).hidden, [true, false, true]);
        parts.isolated = None;
        parts.select(&s, Some(2));
        assert_eq!(look_of(&s, &parts, None, &ExplodeState::default()).highlighted, [true, true, false]);
        // Selecting opens the rows above it.
        assert!(parts.expanded.contains(&1) && parts.expanded.contains(&0));
    }

    #[test]
    fn a_section_lies_across_the_bounds_along_its_plane() {
        let state = SectionState { on: true, plane: Plane::Top, at: 0.25, flip: false };
        assert_eq!(state.section([0.0; 3], [4.0; 3]), Some(Section { normal: [0.0, 1.0, 0.0], offset: 1.0 }));
        let flipped = SectionState { flip: true, ..state };
        assert_eq!(flipped.section([0.0; 3], [4.0; 3]), Some(Section { normal: [-0.0, -1.0, -0.0], offset: -1.0 }));
        assert_eq!(SectionState::default().section([0.0; 3], [4.0; 3]), None);
    }
}
