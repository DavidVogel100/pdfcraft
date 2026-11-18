//! 3D models on the page (PDF 3D annotations, ISO 32000-2 §13.6), shown and turned in place as
//! Acrobat does: the model is read once (on a worker where there are threads), drawn into its
//! rectangle by `pdfcraft_prc_render`, and with the Select tool a drag turns it (Shift, or the
//! right or middle button: moves it), the wheel zooms it, a click picks the part under it (in the
//! Model Tree too) and a double-click goes back to the view it opened on. The Model Tree panel
//! ([`tree`]) hides, shows and isolates parts and turns the section view on.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, pos2};
use pdfcraft_prc_render::{Camera, Frame, Look, RenderOptions, ViewLens, ViewSide, render_look};
use pdfcraft_render::{DocInfo, Format3d};

use crate::QuickTool;
use crate::canvas::PageXform;
use crate::page_cursor::Layer;
use crate::page_router::{Hit, Owner, PagePress};

mod dims;
mod measure;
mod scene;
mod sheet;
mod standard;
#[cfg(test)]
mod tests;
mod tree;

pub(crate) use measure::measure_controls;
use scene::Scene;
pub(crate) use tree::{section_controls, tree_panel};

/// The largest side drawn at rest, in pixels; while the model moves, half of what it would be.
const MAX_SIDE: f32 = 1600.0;
/// How near an edge line a measuring click picks it rather than the face beside it, in points.
const EDGE_REACH: f64 = 5.0;
/// How long after the last move the model is drawn sharp again (seconds).
const SETTLE: f64 = 0.25;
const BACKGROUND: [u8; 4] = [255, 255, 255, 255];

/// A question about making a dimension layer: the model (page, annotation), the part, and how
/// many copies of it the assembly has.
type Ask = ((usize, usize), String, usize, pdfcraft_engine::LayerKind3d);

/// Every 3D model of a document the viewer has met, and this frame's wheel over them.
#[derive(Default)]
pub(crate) struct View3d {
    models: HashMap<(usize, usize), Model>,
    /// Where models were drawn last frame (screen), so the wheel over them can be taken before
    /// the pages scroll.
    rects: Vec<Rect>,
    /// Wheel movement over a model this frame (points; up is positive).
    wheel: f32,
    /// The section view, for every model of the document.
    section: tree::SectionState,
    /// The Measure tool is on: a click on a model picks a face to measure, not a part.
    measuring: bool,
    /// Dimensioning is on: the part selected is isolated and what is measured can be kept on
    /// its dimension layer.
    dimensioning: bool,
    /// Noting is on: the part selected is isolated and a click adds a note to its notes layer.
    noting: bool,
    /// Asking whether to make a dimension layer for a part: which model, the part, its copies.
    ask: Option<Ask>,
    /// An edit to the document made here, for the app to apply.
    edit: Option<pdfcraft_engine::Edit>,
    /// Print part: the model whose sheet is being set up, how, the sheet made (for the app to
    /// open) or why it could not be, and the document's name for its title block.
    sheet_dialog: Option<(usize, usize)>,
    sheet_choice: sheet::SheetChoice,
    sheet: Option<pdfcraft_engine::PartSheet>,
    sheet_error: Option<String>,
    pub(crate) file_name: String,
}

impl View3d {
    /// The edit made here since last asked, for the app to apply.
    pub(crate) fn take_edit(&mut self) -> Option<pdfcraft_engine::Edit> {
        self.edit.take()
    }

    /// The part sheet made since last asked, for the app to open as a new document.
    pub(crate) fn take_sheet(&mut self) -> Option<pdfcraft_engine::PartSheet> {
        self.sheet.take()
    }
}

struct Model {
    load: Load,
    camera: Option<Camera>,
    /// The view it opened on, which a double-click goes back to.
    home: Option<Camera>,
    /// The world axis a drag turns about.
    pivot: [f64; 3],
    texture: Option<egui::TextureHandle>,
    /// The last frame drawn, for picking the part under a click.
    frame: Option<Frame>,
    /// The camera, size and parts revision the texture shows.
    drawn: Option<(Camera, [u32; 2], u64)>,
    /// What the user did to the parts, and the look built from it (at `look_rev`).
    parts: tree::Parts,
    look: Look,
    look_rev: Option<u64>,
    /// Until when the model counts as moving (drawn at half size).
    moving_until: f64,
    /// Where the model was drawn last (screen), for drags to follow the pointer.
    screen: Option<Rect>,
    /// During a drag: whether it moves the model (else it turns it), decided as it starts.
    drag: Option<bool>,
    /// The faces picked to measure, and what they read.
    measure: measure::Measure,
    /// Where the press the model holds went down (screen), seen on the frame it did.
    press_at: Option<Pos2>,
    /// During a turn: the world point it turns about, the model's surface where it started.
    turn_about: Option<[f64; 3]>,
    /// Whether the model is drawn across its whole page, not only in its annotation's box: when
    /// nothing else is drawn on the page around the box, as CAD programs export a model alone.
    whole_page: bool,
    /// The dimension and note layers kept in the document, as read at edit generation
    /// `layers_read`.
    layers: Vec<pdfcraft_engine::Layer3d>,
    layers_read: Option<u64>,
    /// The dimension layer being worked on.
    active: Option<usize>,
    /// Layers the user hid.
    hidden_layers: std::collections::HashSet<usize>,
    /// The dimension picked on the active layer, and one being moved (with where on its text
    /// it was taken).
    dim_picked: Option<usize>,
    dim_drag: Option<(usize, egui::Vec2)>,
    /// Where the active layer's dimension texts were drawn last frame (screen).
    dim_hits: Vec<(usize, Rect)>,
    /// Where the measurement's labels were drawn last frame (screen), and the one being dragged.
    label_hits: Vec<(usize, Rect)>,
    label_drag: Option<usize>,
    /// The node dimensioning last followed, and the part whose new layer to make active.
    dims_node: Option<usize>,
    want_part: Option<(String, pdfcraft_engine::LayerKind3d)>,
    /// The note picked on the active notes layer, one being moved (with where on it it was
    /// taken), and where the notes were drawn last frame (screen).
    note_picked: Option<usize>,
    note_drag: Option<(usize, egui::Vec2)>,
    note_hits: Vec<(usize, Rect)>,
    /// A note being written, not yet added: where it goes and its text so far.
    note_draft: Option<(pdfcraft_engine::NotePlace3d, String)>,
    /// The picked note's text as it is being changed.
    note_text: Option<(usize, String)>,
}

impl Model {
    /// The model's layers with `change` made, as an edit for the app (the layers are read back
    /// from the document once it is applied).
    fn layers_edit(&mut self, page: usize, index: usize, change: impl FnOnce(&mut Vec<pdfcraft_engine::Layer3d>)) -> pdfcraft_engine::Edit {
        let mut layers = self.layers.clone();
        change(&mut layers);
        // Shown at once, before the document is read back.
        self.layers.clone_from(&layers);
        // And as Acrobat shows them.
        let views = match &self.load {
            Load::Ready(scene) => standard::standard_views(scene, &layers, measure::Units::of(scene)),
            _ => Vec::new(),
        };
        pdfcraft_engine::Edit::SetModel3dLayers { page, index, layers, views }
    }

    /// The part isolated, and its own coordinates.
    fn isolated_frame(&self, scene: &Scene) -> Option<(usize, dims::PartFrame)> {
        let node = self.parts.isolated?;
        Some((node, dims::frame_of(scene, node)))
    }
}

enum Load {
    Pending(Receiver<Result<Scene, String>>),
    Ready(Arc<Scene>),
    Failed(String),
}

impl View3d {
    fn ready(&self, page: usize, index: usize) -> bool {
        matches!(self.models.get(&(page, index)).map(|m| &m.load), Some(Load::Ready(_)))
    }

    /// The camera a shown model is seen through.
    #[cfg(test)]
    pub(crate) fn camera(&self, page: usize, index: usize) -> Option<Camera> {
        self.models.get(&(page, index)).and_then(|m| m.camera)
    }
}

/// Before the pages scroll: wheel movement over a model shown last frame is the model's, not the
/// page's. Removes it from this frame's input.
pub(crate) fn take_wheel(ui: &egui::Ui, view: &mut View3d) {
    let rects = std::mem::take(&mut view.rects);
    view.wheel = 0.0;
    let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| rects.iter().any(|r| r.contains(p)));
    if !over {
        return;
    }
    view.wheel = ui.ctx().input_mut(|i| {
        let mut points = 0.0;
        i.events.retain(|e| match e {
            egui::Event::MouseWheel { unit, delta, .. } => {
                points += match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * 40.0,
                    egui::MouseWheelUnit::Page => delta.y * 400.0,
                };
                false
            }
            _ => true,
        });
        i.smooth_scroll_delta = egui::Vec2::ZERO;
        points
    });
}

/// The model under `p` on `page`, for the page router: only once it is shown, and only for the
/// Select tool (the Hand tool pans the page, other tools act on the page as usual).
pub(crate) fn hit(xf: &PageXform, info: &DocInfo, page: usize, tool: QuickTool, view: &View3d, p: Pos2) -> Option<Hit> {
    if tool != QuickTool::Select {
        return None;
    }
    let area = |m: &pdfcraft_render::Annot3d| {
        let whole = view.models.get(&(page, m.index)).is_some_and(|shown| shown.whole_page);
        if whole { xf.rect } else { xf.user_rect(info, page, m.rect) }
    };
    let m = info.annots_3d.iter().rev().find(|m| m.page == page && area(m).contains(p) && view.ready(page, m.index))?;
    let tip = if view.noting {
        tl!("Click the part for a note pointing at it, or beside it for a note on the sheet")
    } else if view.measuring {
        tl!("Click a face to measure it, then another to measure between them")
    } else {
        tl!("Drag to turn · Shift+drag to move · Wheel to zoom · Double-click to reset")
    };
    // Measuring wants a precise point: a thin crosshair, not a hand; a label or a dimension's
    // text can be moved.
    let on_label = view
        .models
        .get(&(page, m.index))
        .is_some_and(|shown| shown.label_hits.iter().chain(&shown.dim_hits).chain(&shown.note_hits).any(|(_, r)| r.contains(p)));
    let icon = if on_label {
        egui::CursorIcon::Move
    } else if view.measuring || view.noting {
        egui::CursorIcon::Crosshair
    } else {
        egui::CursorIcon::Grab
    };
    Some(Hit { tip: Some(tip.into()), ..Hit::new(Owner::Model3d { index: m.index }, Layer::Placed, icon) })
}

/// A press the model holds: drags turn or move it, a click picks a part, a double-click resets
/// the view. Whether it acted.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    page: usize,
    press: PagePress,
    ptr: &crate::page_pointer::PagePointer,
    view: &mut View3d,
    cursor: &mut crate::page_cursor::PageCursor,
) -> bool {
    let PagePress::Mine(Owner::Model3d { index }) = press else { return false };
    let Some(m) = view.models.get_mut(&(page, index)) else { return false };
    // Dimensioning with no part chosen yet: a click picks the part, not a face.
    let measuring = view.measuring && !(view.dimensioning && m.active.is_none());
    let now = ui.input(|i| i.time);
    let height = f64::from(m.screen.map_or(400.0, |r| r.height()));
    // Shift as the key state says, or as this frame's press carried it: the key state can lag
    // the press when Shift went down just before it.
    let shift = ui.input(|i| {
        i.modifiers.shift || i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, modifiers, .. } if modifiers.shift))
    });
    let pivot = m.pivot;
    if let Some(at) = ptr.edges.iter().rev().find_map(|e| match e {
        crate::page_pointer::Edge::Press(p) => Some(*p),
        crate::page_pointer::Edge::Release(_) => None,
    }) {
        m.press_at = Some(at);
    }
    // What a turn falls back to: the middle of the model, never a point off the screen.
    let middle = match &m.load {
        Load::Ready(scene) => scene.bounds.map(|b| b.centre()),
        _ => None,
    };
    let Some(camera) = m.camera.as_mut() else { return true };
    if view.noting && m.active.is_some() {
        return notes_input(ui, resp, ptr, m, (page, index), &mut view.edit, cursor);
    }
    // A measurement's label: dragged away from what it measures, a leader pointing back.
    if ptr.drag_started
        && let Some(at) = m.press_at.or(ptr.origin)
        && let Some(&(k, _)) = m.label_hits.iter().rev().find(|(_, r)| r.contains(at))
    {
        m.label_drag = Some(k);
    }
    if let Some(k) = m.label_drag {
        let d = resp.drag_delta();
        if let Some(slot) = m.measure.moved.get_mut(k) {
            let start = slot.unwrap_or(egui::vec2(8.0, -8.0 + 24.0 * k as f32));
            *slot = Some(start + d);
        }
        if !resp.dragged() || resp.drag_stopped() {
            m.label_drag = None;
        }
        cursor.claim(Layer::Drag, egui::CursorIcon::Move);
        ui.ctx().request_repaint();
        return true;
    }
    if resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| m.label_hits.iter().any(|(_, r)| r.contains(p))) {
        // A click on a label is not a pick through it.
        return true;
    }
    // A dimension's text on the layer being worked on: picked by a click, moved by a drag.
    let on_dim = |at: Pos2| m.dim_hits.iter().rev().find(|(_, r)| r.contains(at)).map(|&(k, r)| (k, at - r.center()));
    if ptr.drag_started
        && m.active.is_some()
        && let Some(hit) = m.press_at.or(ptr.origin).and_then(on_dim)
    {
        m.dim_drag = Some(hit);
        m.dim_picked = Some(hit.0);
    }
    if let Some((k, grab)) = m.dim_drag {
        if let (Some(p), Some(r), Some((drawn_with, size, _)), Load::Ready(scene)) = (resp.interact_pointer_pos(), m.screen, m.drawn, &m.load) {
            let frame = m.parts.isolated.map_or(dims::PartFrame::IDENTITY, |n| dims::frame_of(scene, n));
            if let Some(d) = m.active.and_then(|a| m.layers.get_mut(a)).and_then(|l| l.dims.get_mut(k)) {
                // Moved across the view, at the depth it was.
                let at = p - grab;
                let x = f64::from((at.x - r.left()) / r.width().max(1.0)) * f64::from(size[0]);
                let y = f64::from((at.y - r.top()) / r.height().max(1.0)) * f64::from(size[1]);
                let world = frame.world_of(d.text_at);
                let depth =
                    scene::normalize(scene::sub(drawn_with.target, drawn_with.eye)).map_or(1.0, |f| scene::dot(scene::sub(world, drawn_with.eye), f));
                if let Some(moved) = drawn_with.unproject(x, y, size[0], size[1], depth) {
                    d.text_at = frame.local_of(moved);
                }
            }
        }
        if !resp.dragged() || resp.drag_stopped() {
            m.dim_drag = None;
            view.edit = Some(m.layers_edit(page, index, |_| {}));
        }
        cursor.claim(Layer::Drag, egui::CursorIcon::Move);
        ui.ctx().request_repaint();
        return true;
    }
    if resp.clicked()
        && m.active.is_some()
        && let Some((k, _)) = resp.interact_pointer_pos().and_then(on_dim)
    {
        m.dim_picked = if m.dim_picked == Some(k) { None } else { Some(k) };
        return true;
    }
    if resp.dragged() {
        let other_button = resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle);
        // Decided as the drag starts; Shift pressed partway through still switches to moving.
        let starting = m.drag.is_none();
        let moves = *m.drag.get_or_insert(shift || other_button) || shift;
        m.drag = Some(moves);
        // A turn goes about the model's surface where the press was, as CAD programs turn it;
        // about the middle of the view when the press was beside the model.
        if starting {
            m.turn_about = m.press_at.or(ptr.origin).zip(m.screen).zip(m.frame.as_ref()).and_then(|((p, r), f)| {
                let (w, h) = (f.image.width as f32, f.image.height as f32);
                let x = (p.x - r.left()) / r.width().max(1.0) * w;
                let y = (p.y - r.top()) / r.height().max(1.0) * h;
                // Inside the picture, so the casts are exact.
                (x >= 0.0 && y >= 0.0).then(|| f.point_at(x as u32, y as u32, camera)).flatten()
            });
        }
        let d = resp.drag_delta();
        let (dx, dy) = (f64::from(d.x), f64::from(d.y));
        if moves {
            scene::pan(camera, dx, dy, height);
        } else {
            let about = m.turn_about.or(middle).unwrap_or(camera.target);
            scene::turn(camera, pivot, about, dx, dy, height);
        }
        m.moving_until = now + SETTLE;
        cursor.claim(Layer::Drag, if moves { egui::CursorIcon::Move } else { egui::CursorIcon::Grabbing });
        ui.ctx().request_repaint();
    } else {
        m.drag = None;
    }
    if resp.double_clicked()
        && let Some(home) = m.home
    {
        *camera = home;
    } else if resp.clicked()
        && let (Some(p), Some(r), Load::Ready(scene)) = (resp.interact_pointer_pos(), m.screen, &m.load)
    {
        // What lies under the pointer, from the frame last drawn: the mesh, and the point on it.
        let under = m.frame.as_ref().zip(m.drawn).and_then(|(f, (drawn_with, _, _))| {
            let (w, h) = (f.image.width as f32, f.image.height as f32);
            let at = ((p.x - r.left()) / r.width().max(1.0) * w, (p.y - r.top()) / r.height().max(1.0) * h);
            // Inside the picture, so the casts are exact.
            let (x, y) = (at.0 >= 0.0 && at.1 >= 0.0).then_some((at.0 as u32, at.1 as u32))?;
            Some((f.mesh_at(x, y)?, f.point_at(x, y, &drawn_with)))
        });
        let scene = Arc::clone(scene);
        // An edge line within a few points of the click, as CAD programs pick edges first.
        let edge = if measuring {
            m.frame.as_ref().zip(m.drawn).and_then(|(f, (drawn_with, _, _))| {
                let per_point = f64::from(f.image.width) / f64::from(r.width().max(1.0));
                let at = (f64::from(p.x - r.left()) * per_point, f64::from(p.y - r.top()) * per_point);
                let outlines = m.look.outlines.as_deref()?;
                pdfcraft_prc_render::edges::edge_near(&scene.meshes, outlines, &m.look.hidden, &drawn_with, f, at, EDGE_REACH * per_point)
            })
        } else {
            None
        };
        if measuring {
            // A click beside the model starts the measurement again.
            match (edge, under) {
                (Some((edge, point)), _) => m.measure.pick_edge(&edge, point),
                (None, Some((mesh, Some(point)))) => m.measure.pick(&scene.meshes, mesh, point),
                _ => m.measure.clear(),
            }
        } else {
            let node = under.and_then(|(k, _)| scene.mesh_nodes.get(k).copied().flatten());
            m.parts.select(&scene, node);
        }
    }
    true
}

/// A press on a model while noting: a note's card picked by a click or moved by a drag; a click
/// on the part starts a note with a leader to the point clicked, beside it one on the sheet.
fn notes_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    ptr: &crate::page_pointer::PagePointer,
    m: &mut Model,
    (page, index): (usize, usize),
    edit: &mut Option<pdfcraft_engine::Edit>,
    cursor: &mut crate::page_cursor::PageCursor,
) -> bool {
    let (Some(r), Some((drawn_with, size, _)), Load::Ready(scene), Some(active)) = (m.screen, m.drawn, &m.load, m.active) else { return true };
    let scene = Arc::clone(scene);
    let frame = m.parts.isolated.map_or(dims::PartFrame::IDENTITY, |n| dims::frame_of(&scene, n));
    // A screen point as the picture's pixels.
    let pixel = |p: Pos2| {
        (f64::from((p.x - r.left()) / r.width().max(1.0)) * f64::from(size[0]), f64::from((p.y - r.top()) / r.height().max(1.0)) * f64::from(size[1]))
    };
    let forward = scene::normalize(scene::sub(drawn_with.target, drawn_with.eye)).unwrap_or([0.0, 0.0, -1.0]);
    let depth_of = |world: [f64; 3]| scene::dot(scene::sub(world, drawn_with.eye), forward);
    // A note's card's top left moved to screen point `to`.
    let moved = |place: pdfcraft_engine::NotePlace3d, to: Pos2| match place {
        pdfcraft_engine::NotePlace3d::Leader { anchor, text_at } => {
            let (x, y) = pixel(to);
            let text_at = drawn_with.unproject(x, y, size[0], size[1], depth_of(frame.world_of(text_at))).map_or(text_at, |p| frame.local_of(p));
            pdfcraft_engine::NotePlace3d::Leader { anchor, text_at }
        }
        pdfcraft_engine::NotePlace3d::Sheet { .. } => {
            let at = [
                f64::from(((to.x - r.left()) / r.width().max(1.0)).clamp(0.0, 0.98)),
                f64::from(((to.y - r.top()) / r.height().max(1.0)).clamp(0.0, 0.98)),
            ];
            pdfcraft_engine::NotePlace3d::Sheet { at }
        }
    };
    let on_note = |at: Pos2| m.note_hits.iter().rev().find(|(_, c)| c.contains(at)).map(|&(k, c)| (k, at - c.min));
    if ptr.drag_started
        && let Some(hit) = m.press_at.or(ptr.origin).and_then(on_note)
    {
        m.note_drag = Some(hit);
        m.note_picked = Some(hit.0);
    }
    if let Some((k, grab)) = m.note_drag {
        if let Some(p) = resp.interact_pointer_pos()
            && let Some(note) = m.layers.get_mut(active).and_then(|l| l.notes.get_mut(k))
        {
            note.place = moved(note.place, p - grab);
        }
        if !resp.dragged() || resp.drag_stopped() {
            m.note_drag = None;
            *edit = Some(m.layers_edit(page, index, |_| {}));
        }
        cursor.claim(Layer::Drag, egui::CursorIcon::Move);
        ui.ctx().request_repaint();
        return true;
    }
    if !resp.clicked() {
        return true;
    }
    let Some(p) = resp.interact_pointer_pos() else { return true };
    if let Some((k, _)) = on_note(p) {
        m.note_picked = Some(k);
        m.note_draft = None;
        m.note_text = m.layers.get(active).and_then(|l| l.notes.get(k)).map(|n| (k, n.text.clone()));
        return true;
    }
    m.note_picked = None;
    m.note_text = None;
    // On the part: a note with a leader to the point clicked, its card up and to the right.
    let on_part = m.frame.as_ref().and_then(|f| {
        let (x, y) = pixel(p);
        (x >= 0.0 && y >= 0.0).then(|| f.point_at(x as u32, y as u32, &drawn_with)).flatten()
    });
    let place = match on_part {
        Some(point) => {
            let (x, y) = pixel(p + egui::vec2(40.0, -70.0));
            let text_at = drawn_with.unproject(x, y, size[0], size[1], depth_of(point)).unwrap_or(point);
            pdfcraft_engine::NotePlace3d::Leader { anchor: frame.local_of(point), text_at: frame.local_of(text_at) }
        }
        None => moved(pdfcraft_engine::NotePlace3d::Sheet { at: [0.0, 0.0] }, p),
    };
    let text = m.note_draft.take().map(|(_, t)| t).unwrap_or_default();
    m.note_draft = Some((place, text));
    true
}

/// The page whose models [`paint`] draws.
pub(crate) struct OnPage<'a> {
    pub(crate) xf: &'a PageXform,
    pub(crate) info: &'a DocInfo,
    pub(crate) page: usize,
    pub(crate) doc: &'a pdfcraft_engine::Document,
    /// The page's colour, which the outlines' colours follow.
    pub(crate) paper: Color32,
}

/// Draws the page's models: the model where it is read, else its outline and what is happening.
pub(crate) fn paint(ui: &egui::Ui, on: &OnPage<'_>, view: &mut View3d, pointer: Option<Pos2>) {
    let OnPage { xf, info, page, doc, paper } = *on;
    let ctx = ui.ctx();
    // A model's box can reach past the crop box: nothing is drawn off the page.
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(xf.rect));
    let now = ui.input(|i| i.time);
    let ppp = ctx.pixels_per_point();
    for model in info.annots_3d.iter().filter(|m| m.page == page) {
        let r = xf.user_rect(info, page, model.rect);
        if !r.is_positive() || !r.intersects(painter.clip_rect()) {
            continue;
        }
        if model.format != Format3d::Prc {
            let what =
                if model.format == Format3d::U3d { tl!("U3D models can't be shown yet") } else { tl!("This 3D model's format isn't supported") };
            label(&painter, r, paper, &[tl!("3D model").to_owned(), what.to_owned()]);
            continue;
        }
        let key = (page, model.index);
        let m = view.models.entry(key).or_insert_with(|| Model {
            load: start_load(ctx, doc, page, model.index),
            camera: None,
            home: None,
            pivot: [0.0, 0.0, 1.0],
            texture: None,
            frame: None,
            drawn: None,
            parts: tree::Parts::default(),
            look: Look::default(),
            look_rev: None,
            moving_until: 0.0,
            screen: None,
            drag: None,
            measure: measure::Measure::default(),
            press_at: None,
            turn_about: None,
            whole_page: alone_on_page(doc, info, page, model),
            layers: Vec::new(),
            layers_read: None,
            active: None,
            hidden_layers: std::collections::HashSet::new(),
            dim_picked: None,
            dim_drag: None,
            dim_hits: Vec::new(),
            label_hits: Vec::new(),
            label_drag: None,
            dims_node: None,
            want_part: None,
            note_picked: None,
            note_drag: None,
            note_hits: Vec::new(),
            note_draft: None,
            note_text: None,
        });
        // The area the model is drawn in: its box, or its whole page.
        let box_rect = r;
        let r = if m.whole_page { xf.rect } else { r };
        if let Load::Pending(rx) = &m.load {
            match rx.try_recv() {
                Ok(Ok(scene)) => m.load = Load::Ready(Arc::new(scene)),
                Ok(Err(e)) => m.load = Load::Failed(e),
                Err(TryRecvError::Disconnected) => m.load = Load::Failed(tl!("the model could not be read").to_owned()),
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let scene = match &m.load {
            Load::Pending(_) => {
                label(&painter, r, paper, &[tl!("Loading 3D model…").to_owned()]);
                continue;
            }
            Load::Failed(e) => {
                label(&painter, r, paper, &[tl!("This 3D model can't be shown").to_owned(), e.clone()]);
                continue;
            }
            Load::Ready(scene) => Arc::clone(scene),
        };
        let Some(bounds) = scene.bounds else {
            label(&painter, r, paper, &[tl!("This 3D model can't be shown").to_owned(), tl!("the model is empty").to_owned()]);
            continue;
        };
        // The view the PDF opens the model on, else a three-quarter view of all of it.
        if m.camera.is_none() {
            let size = [f64::from((model.rect[2] - model.rect[0]).abs()), f64::from((model.rect[3] - model.rect[1]).abs())];
            let aspect = if size[1] > 0.0 { size[0] / size[1] } else { 1.0 };
            let saved =
                model.view.and_then(|v| Camera::from_view(v.camera_to_world, v.center_of_orbit, lens(v.projection), size, bounds.centre()).ok());
            let fitted = || Camera::fit_meshes(&scene.meshes, [-1.0, 1.0, -1.0], [0.0, 0.0, 1.0], true, aspect).ok();
            if let Some(mut c) = saved.or_else(fitted) {
                // Across the whole page, the model keeps the size and place it has in its box.
                if m.whole_page && box_rect.height() > 0.0 {
                    scene::widen(&mut c, f64::from(r.height() / box_rect.height()));
                    let off = box_rect.center() - r.center();
                    scene::pan(&mut c, f64::from(off.x), f64::from(off.y), f64::from(r.height()));
                }
                m.pivot = scene::nearest_axis(c.up);
                m.camera = Some(c);
                m.home = Some(c);
            }
        }
        let kind = if view.dimensioning {
            Some(pdfcraft_engine::LayerKind3d::Dimensions)
        } else if view.noting {
            Some(pdfcraft_engine::LayerKind3d::Notes)
        } else {
            None
        };
        if layers_step(ctx, doc, m, &scene, (page, model.index), (kind, &mut view.ask, &mut view.edit)) {
            // Cancelled: no layer, so neither tool goes on.
            view.dimensioning = false;
            view.noting = false;
        }
        let Some(mut camera) = m.camera else {
            label(&painter, r, paper, &[tl!("This 3D model can't be shown").to_owned(), tl!("its view can't be formed").to_owned()]);
            continue;
        };
        let background = model.view.and_then(|v| v.background).map_or(BACKGROUND, |c| {
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [byte(c[0]), byte(c[1]), byte(c[2]), 255]
        });
        // The wheel over this model zooms it (up: closer).
        // The wheel over this model zooms it (up: closer), towards what is under the pointer: the
        // model's surface there, else the point level with the middle of the view.
        if let Some(p) = pointer.filter(|p| view.wheel != 0.0 && r.contains(*p)) {
            let about = m.frame.as_ref().and_then(|f| {
                let (w, h) = (f.image.width, f.image.height);
                let x = f64::from((p.x - r.left()) / r.width().max(1.0)) * f64::from(w);
                let y = f64::from((p.y - r.top()) / r.height().max(1.0)) * f64::from(h);
                // Inside the picture, so the casts are exact.
                let on_model = (x >= 0.0 && y >= 0.0).then(|| f.point_at(x as u32, y as u32, &camera)).flatten();
                on_model.or_else(|| camera.unproject(x, y, w, h, scene::depth_of_target(&camera)))
            });
            let about = about.unwrap_or(camera.target);
            scene::zoom_at(&mut camera, about, f64::from(-view.wheel * 0.002).exp());
            m.camera = Some(camera);
            m.moving_until = now + SETTLE;
            view.wheel = 0.0;
        }
        let moving = now < m.moving_until;
        if moving {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
        }
        let scale = (ppp * if moving { 0.5 } else { 1.0 }).min(MAX_SIDE / r.width().max(r.height()).max(1.0));
        // At most MAX_SIDE a side, so the casts are exact.
        let size = [(r.width() * scale).round().max(1.0) as u32, (r.height() * scale).round().max(1.0) as u32];
        if m.look_rev != Some(m.parts.rev) {
            m.look = tree::look_of(&scene, &m.parts, view.section.section(bounds.min, bounds.max));
            m.look_rev = Some(m.parts.rev);
        }
        if m.drawn != Some((camera, size, m.parts.rev)) {
            let options = RenderOptions { width: size[0], height: size[1], background, ..RenderOptions::default() };
            match render_look(&scene.meshes, &camera, &options, &m.look) {
                Ok(frame) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([size[0] as usize, size[1] as usize], &frame.image.rgba);
                    match &mut m.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => m.texture = Some(ctx.load_texture(format!("view3d-{page}-{}", model.index), image, egui::TextureOptions::LINEAR)),
                    }
                    m.frame = Some(frame);
                }
                Err(e) => {
                    label(&painter, r, paper, &[tl!("This 3D model can't be shown").to_owned(), e.to_string()]);
                    continue;
                }
            }
            m.drawn = Some((camera, size, m.parts.rev));
        }
        if let Some(t) = &m.texture {
            painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        paint_overlays(&painter, r, m, &scene, (view.dimensioning, view.noting));
        // While a drag is on, say which it does: the system draws the same cursor for both on
        // Windows, so the frame and a corner label show it.
        if let Some(moves) = m.drag {
            let accent = if moves { Color32::from_rgb(0x1B, 0x63, 0xE0) } else { Color32::from_rgb(0x2E, 0x9E, 0x5B) };
            painter.rect_stroke(r.shrink(1.0), CornerRadius::same(2), Stroke::new(2.0, accent), egui::StrokeKind::Inside);
            let text = if moves { tl!("Moving") } else { tl!("Turning") };
            let at = pos2(r.left() + 8.0, r.top() + 8.0);
            let galley = painter.layout_no_wrap(text.to_owned(), crate::theme::semibold(12.0), Color32::WHITE);
            painter.rect_filled(Rect::from_min_size(at, galley.size()).expand(4.0), CornerRadius::same(3), accent);
            painter.galley(at, galley, Color32::WHITE);
        }
        m.screen = Some(r);
        view.rects.push(r);
    }
}

/// Each frame, before the model is drawn: its layers read again after an edit, the part
/// selected followed while dimensioning, and whether to make a layer for it asked.
fn layers_step(
    ctx: &egui::Context,
    doc: &pdfcraft_engine::Document,
    m: &mut Model,
    scene: &Scene,
    key: (usize, usize),
    (kind, asking, edit): (Option<pdfcraft_engine::LayerKind3d>, &mut Option<Ask>, &mut Option<pdfcraft_engine::Edit>),
) -> bool {
    // The layers kept in the document, read again after every edit (an undo too).
    let generation = doc.edit_generation();
    if m.layers_read != Some(generation) && m.dim_drag.is_none() && m.note_drag.is_none() {
        m.layers = doc.model_3d_layers(key.0, key.1).unwrap_or_default();
        m.layers_read = Some(generation);
        m.active = m.active.filter(|&a| a < m.layers.len());
        if let Some((part, kind)) = m.want_part.take() {
            m.active = dims::layer_for(&m.layers, &part, kind);
        }
        let active = m.active.and_then(|a| m.layers.get(a));
        let (dims_count, notes_count) = active.map_or((0, 0), |l| (l.dims.len(), l.notes.len()));
        m.dim_picked = m.dim_picked.filter(|&k| k < dims_count);
        m.note_picked = m.note_picked.filter(|&k| k < notes_count);
        m.note_text = m.note_text.take().filter(|(k, _)| *k < notes_count);
    }
    if let Some(question) = follow_selection(kind, m, scene, key) {
        *asking = Some(question);
    }
    let mut cancelled = false;
    if let Some((_, part, copies, kind)) = asking.as_ref().filter(|(k, ..)| *k == key)
        && let Some(answer) = ask_for_layer(ctx, (part.as_str(), *copies, *kind))
    {
        let (part, kind) = (part.clone(), *kind);
        *asking = None;
        if answer {
            let frame = m.isolated_frame(scene).map_or(dims::PartFrame::IDENTITY, |(_, f)| f);
            let view_saved = m.camera.map(|c| frame.save_view(&c));
            let name = if kind == pdfcraft_engine::LayerKind3d::Notes { tl!("Notes") } else { tl!("Dimensions") };
            let layer = pdfcraft_engine::Layer3d { part: part.clone(), kind, name: name.to_owned(), view: view_saved, ..Default::default() };
            *edit = Some(m.layers_edit(key.0, key.1, |l| l.push(layer)));
            m.active = dims::layer_for(&m.layers, &part, kind);
            m.want_part = Some((part, kind));
        } else {
            cancelled = true;
        }
    }
    cancelled
}

/// Over the model drawn in `r`: the isolated part's layers (the one worked on can be picked and
/// moved) and the measurement.
fn paint_overlays(painter: &egui::Painter, r: Rect, m: &mut Model, scene: &Scene, (dimensioning, noting): (bool, bool)) {
    m.dim_hits.clear();
    m.note_hits.clear();
    if let Some((drawn_with, drawn_size, _)) = m.drawn {
        let units = measure::Units::of(scene);
        // The isolated part's layers.
        if let Some((node, frame)) = m.isolated_frame(scene) {
            let part = dims::part_name(scene, node);
            let copies = dims::copies(scene, &part);
            for (k, layer) in m.layers.iter().enumerate() {
                if layer.part != part || m.hidden_layers.contains(&k) {
                    continue;
                }
                if layer.kind == pdfcraft_engine::LayerKind3d::Notes {
                    let active = noting && m.active == Some(k);
                    let draft = if active { m.note_draft.as_ref() } else { None };
                    let notes = dims::NotesShown { layer, picked: if active { m.note_picked } else { None }, draft };
                    let hits = dims::paint_notes(painter, r, drawn_size, &drawn_with, &frame, &notes);
                    if active {
                        m.note_hits = hits;
                    }
                    continue;
                }
                let active = dimensioning && m.active == Some(k);
                let shown = dims::Shown { layer, active, picked: if active { m.dim_picked } else { None }, copies };
                let hits = dims::paint(painter, r, drawn_size, &drawn_with, &frame, &shown, units);
                if active {
                    m.dim_hits = hits;
                }
            }
        }
        m.label_hits = measure::paint(painter, r, drawn_size, &drawn_with, &m.measure, units);
    }
}

/// While dimensioning: the part selected is isolated and its dimension layer made the one worked
/// on, or, when it has none, whether to make one is asked (the model, the part, its copies).
fn follow_selection(kind: Option<pdfcraft_engine::LayerKind3d>, m: &mut Model, scene: &Scene, key: (usize, usize)) -> Option<Ask> {
    let Some(kind) = kind else {
        m.dims_node = None;
        m.active = None;
        m.dim_picked = None;
        m.note_picked = None;
        m.note_draft = None;
        m.note_text = None;
        return None;
    };
    let node = m.parts.selected?;
    if m.dims_node == Some(node) {
        return None;
    }
    m.dims_node = Some(node);
    if m.parts.isolated != Some(node) {
        m.parts.isolated = Some(node);
        let under = scene.subtree(node);
        m.parts.hidden.retain(|h| !under.contains(h));
        m.parts.changed();
    }
    m.measure.clear();
    m.dim_picked = None;
    m.note_picked = None;
    m.note_draft = None;
    m.note_text = None;
    let part = dims::part_name(scene, node);
    match dims::layer_for(&m.layers, &part, kind) {
        Some(k) => {
            m.active = Some(k);
            // Turned to the view the layer was made in.
            if let Some(v) = m.layers.get(k).and_then(|l| l.view) {
                m.camera = Some(dims::frame_of(scene, node).camera(&v));
            }
            None
        }
        None => {
            m.active = None;
            if let Some(c) = m.camera.as_mut() {
                fit_part(c, scene, node);
            }
            let copies = dims::copies(scene, &part);
            Some((key, part, copies, kind))
        }
    }
}

/// Turns nothing but moves `camera` in or out and across so `node`'s part fills the view.
fn fit_part(camera: &mut Camera, scene: &Scene, node: usize) {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for k in scene.subtree(node) {
        for &mi in scene.nodes.get(k).map_or(&[][..], |n| n.meshes.as_slice()) {
            for p in scene.meshes.get(mi).map_or(&[][..], |m| m.mesh.positions.as_slice()) {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
        }
    }
    if !(0..3).all(|a| lo[a].is_finite() && hi[a].is_finite()) {
        return;
    }
    let centre = scene::scale(scene::add(lo, hi), 0.5);
    let half = scene::sub(hi, lo);
    let radius = (scene::dot(half, half).sqrt() / 2.0).max(1e-9);
    let Some(forward) = scene::normalize(scene::sub(camera.target, camera.eye)) else { return };
    let distance = match &mut camera.projection {
        pdfcraft_prc_render::Projection::Perspective { fov_y } => radius * 1.15 / (fov_y.to_radians() / 2.0).sin(),
        pdfcraft_prc_render::Projection::Orthographic { height } => {
            *height = radius * 2.3;
            radius * 4.0
        }
    };
    camera.target = centre;
    camera.eye = scene::sub(centre, scene::scale(forward, distance));
}

/// Asks whether to make a layer of `kind` for `part` (with `copies` copies): `Some(true)` for
/// Yes, `Some(false)` for Cancel, `None` while it is open.
fn ask_for_layer(ctx: &egui::Context, (part, copies, kind): (&str, usize, pdfcraft_engine::LayerKind3d)) -> Option<bool> {
    let mut answer = None;
    let modal = crate::widgets::show_modal(ctx, "view3d-dimension-layer", |ui| {
        ui.set_width(380.0);
        let question = if kind == pdfcraft_engine::LayerKind3d::Notes {
            crate::i18n::fmt(tl!("Create a Notes layer for {part}?"), &[("part", part)])
        } else {
            crate::i18n::fmt(tl!("Create a Dimension layer for {part}?"), &[("part", part)])
        };
        ui.add(egui::Label::new(egui::RichText::new(question).font(crate::theme::semibold(15.0))).wrap());
        if copies > 1 {
            ui.add_space(4.0);
            let note = crate::i18n::fmt(tl!("It will show on all {n} copies of this part."), &[("n", &copies.to_string())]);
            ui.label(note);
        }
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::pill_button(ui, tl!("Yes"), true).clicked() {
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

/// Whether `model` is the only thing on its page: no other annotation or link there, and every
/// piece of the page's own content inside the model's box (a poster, as CAD programs write one).
fn alone_on_page(doc: &pdfcraft_engine::Document, info: &DocInfo, page: usize, model: &pdfcraft_render::Annot3d) -> bool {
    // A little room for a poster's edge drawn on the box's line.
    const SLACK: f64 = 2.0;
    let [x0, y0, x1, y1] = model.rect.map(f64::from);
    let (x0, x1, y0, y1) = (x0.min(x1) - SLACK, x0.max(x1) + SLACK, y0.min(y1) - SLACK, y0.max(y1) + SLACK);
    let others = info.annots_3d.iter().any(|m| m.page == page && m.index != model.index)
        || info.annotations.iter().any(|a| a.page == page)
        || info.links.iter().any(|l| l.page == page);
    if others {
        return false;
    }
    doc.editable_objects(page).is_ok_and(|objects| objects.iter().all(|o| o.rect[0] >= x0 && o.rect[2] <= x1 && o.rect[1] >= y0 && o.rect[3] <= y1))
}

/// A PDF 3D view's projection as the renderer's lens.
fn lens(p: pdfcraft_render::Projection3d) -> ViewLens {
    let side = |d: pdfcraft_render::Dimension3d| match d {
        pdfcraft_render::Dimension3d::Width => ViewSide::Width,
        pdfcraft_render::Dimension3d::Height => ViewSide::Height,
        pdfcraft_render::Dimension3d::Min => ViewSide::Min,
        pdfcraft_render::Dimension3d::Max => ViewSide::Max,
    };
    match p {
        pdfcraft_render::Projection3d::Perspective { fov, scaled } => ViewLens::Perspective { fov, side: side(scaled) },
        pdfcraft_render::Projection3d::Orthographic { scale, scaled } => ViewLens::Orthographic { scale, side: side(scaled) },
    }
}

/// Reads the model: its data from the document here, the PRC on a worker (inline on the web).
fn start_load(ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize, index: usize) -> Load {
    let (tx, rx) = std::sync::mpsc::channel();
    let model = match doc.model_3d(page, index) {
        Ok(m) => m,
        Err(e) => return Load::Failed(e),
    };
    if model.kind != pdfcraft_engine::model3d::Model3dKind::Prc {
        return Load::Failed(tl!("only PRC models can be shown").to_owned());
    }
    let ctx = ctx.clone();
    let work = move || {
        // The receiver is gone when the document closed while reading: nothing to tell.
        let _ = tx.send(Scene::read(&model.data));
        ctx.request_repaint();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Err(e) = std::thread::Builder::new().name("view3d-load".into()).spawn(work) {
            return Load::Failed(e.to_string());
        }
    }
    #[cfg(target_arch = "wasm32")]
    work();
    Load::Pending(rx)
}

/// The outline of a model that isn't shown, with what is happening (or why not). Drawn over the
/// page's `paper` (white, or the reading colours' background), so the colours follow it rather
/// than the theme.
fn label(painter: &egui::Painter, r: Rect, paper: Color32, lines: &[String]) {
    let dark = u16::from(paper.r()) + u16::from(paper.g()) + u16::from(paper.b()) < 3 * 128;
    let (accent, note) = if dark {
        (Color32::from_rgb(0x7F, 0xB0, 0xFF), Color32::from_gray(0xC0))
    } else {
        (Color32::from_rgb(0x1B, 0x63, 0xE0), Color32::from_gray(0x55))
    };
    painter.rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0, accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
    // Too small for text: the outline alone says where the model is.
    if r.width() < 120.0 || r.height() < 48.0 {
        return;
    }
    let painter = painter.with_clip_rect(painter.clip_rect().intersect(r));
    let size = (r.height() / 12.0).clamp(12.0, 22.0);
    let top = r.center().y - (lines.len() as f32 - 1.0) * size * 0.7;
    for (k, line) in lines.iter().enumerate() {
        let (font, color) = if k == 0 { (crate::theme::semibold(size), accent) } else { (crate::theme::regular(size * 0.7), note) };
        let at = pos2(r.center().x, top + k as f32 * size * 1.4);
        painter.text(at, egui::Align2::CENTER_CENTER, line, font, color);
    }
}
