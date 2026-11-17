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

mod measure;
mod scene;
#[cfg(test)]
mod tests;
mod tree;

pub(crate) use measure::measure_controls;
use scene::Scene;
pub(crate) use tree::{section_controls, tree_panel};

/// The largest side drawn at rest, in pixels; while the model moves, half of what it would be.
const MAX_SIDE: f32 = 1600.0;
/// How long after the last move the model is drawn sharp again (seconds).
const SETTLE: f64 = 0.25;
const BACKGROUND: [u8; 4] = [255, 255, 255, 255];

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
    /// During a turn: the world point it turns about, the model's surface where it started.
    turn_about: Option<[f64; 3]>,
    /// Whether the model is drawn across its whole page, not only in its annotation's box: when
    /// nothing else is drawn on the page around the box, as CAD programs export a model alone.
    whole_page: bool,
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
    let tip = if view.measuring {
        tl!("Click a face to measure it, then another to measure between them")
    } else {
        tl!("Drag to turn · Shift+drag to move · Wheel to zoom · Double-click to reset")
    };
    Some(Hit { tip: Some(tip.into()), ..Hit::new(Owner::Model3d { index: m.index }, Layer::Placed, egui::CursorIcon::Grab) })
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
    let measuring = view.measuring;
    let Some(m) = view.models.get_mut(&(page, index)) else { return false };
    let now = ui.input(|i| i.time);
    let height = f64::from(m.screen.map_or(400.0, |r| r.height()));
    // Shift as the key state says, or as this frame's press carried it: the key state can lag
    // the press when Shift went down just before it.
    let shift = ui.input(|i| {
        i.modifiers.shift || i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, modifiers, .. } if modifiers.shift))
    });
    let pivot = m.pivot;
    let Some(camera) = m.camera.as_mut() else { return true };
    if resp.dragged() {
        let other_button = resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle);
        // Decided as the drag starts; Shift pressed partway through still switches to moving.
        let starting = m.drag.is_none();
        let moves = *m.drag.get_or_insert(shift || other_button) || shift;
        m.drag = Some(moves);
        // A turn goes about the model's surface where the press was, as CAD programs turn it;
        // about the middle of the view when the press was beside the model.
        if starting {
            m.turn_about = ptr.origin.zip(m.screen).zip(m.frame.as_ref()).and_then(|((p, r), f)| {
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
            let about = m.turn_about.unwrap_or(camera.target);
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
        if measuring {
            // A click beside the model starts the measurement again.
            match under {
                Some((mesh, Some(point))) => m.measure.pick(&scene.meshes, mesh, point),
                _ => m.measure.clear(),
            }
        } else {
            let node = under.and_then(|(k, _)| scene.mesh_nodes.get(k).copied().flatten());
            m.parts.select(&scene, node);
        }
    }
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
            turn_about: None,
            whole_page: alone_on_page(doc, info, page, model),
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
        let Some(mut camera) = m.camera else {
            label(&painter, r, paper, &[tl!("This 3D model can't be shown").to_owned(), tl!("its view can't be formed").to_owned()]);
            continue;
        };
        let background = model.view.and_then(|v| v.background).map_or(BACKGROUND, |c| {
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [byte(c[0]), byte(c[1]), byte(c[2]), 255]
        });
        // The wheel over this model zooms it (up: closer).
        if view.wheel != 0.0 && pointer.is_some_and(|p| r.contains(p)) {
            scene::zoom(&mut camera, f64::from(-view.wheel * 0.002).exp());
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
        if let Some((drawn_with, drawn_size, _)) = m.drawn {
            measure::paint(&painter, r, drawn_size, &drawn_with, &m.measure, measure::Units::of(&scene));
        }
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
