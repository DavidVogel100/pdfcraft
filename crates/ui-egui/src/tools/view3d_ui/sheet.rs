//! Print part: the isolated part on a sheet of paper, seen square-on (a parallel view, so
//! sizes on the paper keep their proportions), with its shown dimension and note layers drawn
//! over the picture as vectors and a title block. Made into a new PDF to print or save
//! (`pdfcraft_engine::part_sheet`).

use egui::{Pos2, Vec2, pos2, vec2};
use pdfcraft_engine::{DimKind3d, LayerKind3d, NotePlace3d, PartSheet, SheetMark, SheetPicture};
use pdfcraft_prc_render::{Camera, Projection, RenderOptions, render_look};

use super::dims;
use super::measure::Units;
use super::scene::{Scene, dot, sub};

/// A sheet size, named as the paper is sold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Paper {
    #[default]
    Letter,
    Tabloid,
    A4,
    A3,
    ArchC,
    ArchD,
}

impl Paper {
    pub(crate) const ALL: [Paper; 6] = [Paper::Letter, Paper::Tabloid, Paper::A4, Paper::A3, Paper::ArchC, Paper::ArchD];

    /// Short side and long side, in points.
    fn size(self) -> (f64, f64) {
        match self {
            Paper::Letter => (612.0, 792.0),
            Paper::Tabloid => (792.0, 1224.0),
            Paper::A4 => (595.3, 841.9),
            Paper::A3 => (841.9, 1190.6),
            Paper::ArchC => (1296.0, 1728.0),
            Paper::ArchD => (1728.0, 2592.0),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Paper::Letter => "Letter (8.5 × 11 in)",
            Paper::Tabloid => "Tabloid (11 × 17 in)",
            Paper::A4 => "A4",
            Paper::A3 => "A3",
            Paper::ArchC => "Arch C (18 × 24 in)",
            Paper::ArchD => "Arch D (24 × 36 in)",
        }
    }
}

/// How the sheet is made, as last chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SheetChoice {
    pub(crate) paper: Paper,
    pub(crate) landscape: bool,
    pub(crate) title_block: bool,
}

impl Default for SheetChoice {
    fn default() -> Self {
        Self { paper: Paper::Letter, landscape: true, title_block: true }
    }
}

const MARGIN: f64 = 36.0;
const TITLE_HEIGHT: f64 = 48.0;
/// The picture's resolution, and the most pixels it may have.
const DPI: f64 = 200.0;
const MAX_PIXELS: f64 = 12.0e6;
const BLACK: [u8; 3] = [0x10, 0x10, 0x14];
const GREY: [u8; 3] = [0x70, 0x70, 0x78];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
const NOTE_FILL: [u8; 3] = [0xFF, 0xF8, 0xD6];
const TEXT_SIZE: f64 = 9.0;
const NOTE_WIDTH: f64 = 170.0;

/// What the sheet is of: the part, how it is seen on screen, and what is said about it.
pub(crate) struct Subject<'a> {
    pub(crate) scene: &'a Scene,
    pub(crate) node: usize,
    pub(crate) camera: Camera,
    /// The view's width over its height on screen.
    pub(crate) aspect: f64,
    pub(crate) look: &'a pdfcraft_prc_render::Look,
    /// The layers to draw (those of the part, shown).
    pub(crate) layers: Vec<&'a pdfcraft_engine::Layer3d>,
    pub(crate) units: Units,
    pub(crate) file: &'a str,
    pub(crate) date: String,
}

/// The sheet for `subject` as `choice` says.
pub(crate) fn make(subject: &Subject<'_>, choice: SheetChoice) -> Result<PartSheet, String> {
    let (short, long) = choice.paper.size();
    let (w, h) = if choice.landscape { (long, short) } else { (short, long) };
    let bottom = if choice.title_block { MARGIN + TITLE_HEIGHT + 8.0 } else { MARGIN };
    let rect = [MARGIN, bottom, w - MARGIN, h - MARGIN];
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    // Seen square-on, showing at least what the screen shows.
    let c = subject.camera;
    let distance = dot(sub(c.target, c.eye), sub(c.target, c.eye)).sqrt();
    let seen = match c.projection {
        Projection::Orthographic { height } => height,
        Projection::Perspective { fov_y } => 2.0 * distance * (fov_y.to_radians() / 2.0).tan(),
    };
    let height = seen.max(seen * subject.aspect / (rw / rh));
    let camera = Camera { projection: Projection::Orthographic { height }, ..c };
    // The picture's pixels: DPI, fewer on big paper.
    let mut scale = DPI / 72.0;
    if rw * rh * scale * scale > MAX_PIXELS {
        scale = (MAX_PIXELS / (rw * rh)).sqrt();
    }
    // Within MAX_PIXELS, so the casts are exact.
    let (pw, ph) = ((rw * scale).round().max(1.0) as u32, (rh * scale).round().max(1.0) as u32);
    let mut look = subject.look.clone();
    look.highlighted = vec![false; look.highlighted.len()];
    let options = RenderOptions { width: pw, height: ph, background: [255, 255, 255, 255], ..RenderOptions::default() };
    let frame = render_look(&subject.scene.meshes, &camera, &options, &look).map_err(|e| e.to_string())?;
    // Page points, measured down from the top of the page as the screen is.
    let place = dims::frame_of(subject.scene, subject.node);
    let to_page = |p: [f64; 3]| -> Option<Pos2> {
        let [x, y] = camera.project(place.world_of(p), pw, ph)?;
        Some(pos2((rect[0] + x / f64::from(pw) * rw) as f32, ((h - rect[3]) + y / f64::from(ph) * rh) as f32))
    };
    let mut marks = Marks { h, out: Vec::new() };
    for layer in &subject.layers {
        match layer.kind {
            LayerKind3d::Dimensions => dimensions(&mut marks, layer, &to_page, subject.units),
            LayerKind3d::Notes => notes(&mut marks, layer, &to_page, [rect[0], h - rect[3], rw, rh]),
        }
    }
    // The drawing's frame, and the title block under it.
    marks.out.insert(0, SheetMark::Box { rect, fill: None, stroke: Some((BLACK, 0.8)) });
    let part = dims::part_name(subject.scene, subject.node);
    if choice.title_block {
        title_block(&mut marks.out, [MARGIN, MARGIN, w - MARGIN, MARGIN + TITLE_HEIGHT], &part, subject);
    }
    Ok(PartSheet { page: (w, h), picture: SheetPicture { width: pw, height: ph, rgba: frame.image.rgba, rect }, marks: marks.out, title: part })
}

/// Marks being drawn, given in points down from the top of a page `h` tall.
struct Marks {
    h: f64,
    out: Vec<SheetMark>,
}

impl Marks {
    fn up(&self, p: Pos2) -> [f64; 2] {
        [f64::from(p.x), self.h - f64::from(p.y)]
    }

    fn line(&mut self, a: Pos2, b: Pos2) {
        let (from, to) = (self.up(a), self.up(b));
        self.out.push(SheetMark::Line { from, to, width: 0.6, colour: BLACK });
    }

    fn arrow(&mut self, tip: Pos2, dir: Vec2) {
        let back = tip - dir * 7.0;
        let side = vec2(-dir.y, dir.x) * 2.2;
        let points = vec![self.up(tip), self.up(back + side), self.up(back - side)];
        self.out.push(SheetMark::Fill { points, colour: BLACK });
    }

    /// One line of text centred on `centre`, on a white ground.
    fn label(&mut self, centre: Pos2, text: &str) {
        let width = pdfcraft_fonts::helvetica_width(text, TEXT_SIZE) as f32;
        let size = vec2(width, TEXT_SIZE as f32);
        let min = centre - size / 2.0;
        let [x0, y1] = self.up(min);
        let [x1, y0] = self.up(min + size);
        self.out.push(SheetMark::Box { rect: [x0 - 2.0, y0 - 2.5, x1 + 2.0, y1 + 1.5], fill: Some(WHITE), stroke: None });
        self.out.push(SheetMark::Text { at: [x0, y0 + 1.0], size: TEXT_SIZE, text: text.to_owned(), colour: BLACK });
    }
}

fn dimensions(marks: &mut Marks, layer: &pdfcraft_engine::Layer3d, to_page: &impl Fn([f64; 3]) -> Option<Pos2>, units: Units) {
    for d in &layer.dims {
        let points: Vec<Pos2> = d.points.iter().filter_map(|&p| to_page(p)).collect();
        let Some(text_at) = to_page(d.text_at) else { continue };
        match (d.kind, points.as_slice()) {
            (DimKind3d::Distance, &[a, b]) => {
                let along = (b - a).normalized();
                if along.is_finite() && along.length() > 0.5 {
                    let across = vec2(-along.y, along.x);
                    let off = across * (text_at - a).dot(across);
                    let (da, db) = (a + off, b + off);
                    if off.length() > 3.0 {
                        let (gap, over) = (off.normalized() * 2.0, off.normalized() * 3.0);
                        marks.line(a + gap, da + over);
                        marks.line(b + gap, db + over);
                    }
                    marks.line(da, db);
                    marks.arrow(da, -along);
                    marks.arrow(db, along);
                    let t = (text_at - da).dot(along);
                    if t < 0.0 {
                        marks.line(da, da + along * t);
                    } else if t > (db - da).length() {
                        marks.line(db, da + along * t);
                    }
                }
            }
            (DimKind3d::Diameter, &[c, at]) => {
                let dir = (at - c).normalized();
                if dir.is_finite() {
                    let far = c - (at - c);
                    marks.line(far, at);
                    marks.arrow(at, dir);
                    marks.arrow(far, -dir);
                    leader(marks, at, text_at);
                }
            }
            (DimKind3d::Radius, &[c, at]) => {
                let dir = (at - c).normalized();
                if dir.is_finite() {
                    marks.line(c, at);
                    marks.arrow(at, dir);
                    leader(marks, at, text_at);
                }
            }
            (_, &[at, ..]) => leader(marks, at, text_at),
            _ => {}
        }
        marks.label(text_at, &dims::dim_text(d, units));
    }
}

fn leader(marks: &mut Marks, from: Pos2, to: Pos2) {
    let knee = pos2(to.x + if to.x >= from.x { -10.0 } else { 10.0 }, to.y);
    marks.line(from, knee);
    marks.line(knee, to);
}

/// The layer's notes: each in a card, with a leader to what it is about; `view` is the
/// picture's left, top, width and height (points down from the page's top).
fn notes(marks: &mut Marks, layer: &pdfcraft_engine::Layer3d, to_page: &impl Fn([f64; 3]) -> Option<Pos2>, view: [f64; 4]) {
    for note in &layer.notes {
        let (at, anchor) = match note.place {
            NotePlace3d::Leader { anchor, text_at } => {
                let Some(at) = to_page(text_at) else { continue };
                (at, to_page(anchor))
            }
            NotePlace3d::Sheet { at } => (pos2((view[0] + at[0] * view[2]) as f32, (view[1] + at[1] * view[3]) as f32), None),
        };
        let lines = pdfcraft_fonts::wrap_with(&note.text, TEXT_SIZE, NOTE_WIDTH, pdfcraft_fonts::helvetica_width);
        let width = lines.iter().map(|l| pdfcraft_fonts::helvetica_width(l, TEXT_SIZE)).fold(0.0, f64::max) as f32;
        let line_height = (TEXT_SIZE * 1.25) as f32;
        let card = egui::Rect::from_min_size(at, vec2(width + 8.0, line_height * lines.len() as f32 + 6.0));
        if let Some(tip) = anchor {
            let from = pos2(tip.x.clamp(card.left(), card.right()), tip.y.clamp(card.top(), card.bottom()));
            let d = tip - from;
            if d.length() > 8.0 {
                let dir = d.normalized();
                marks.line(from, tip - dir * 5.0);
                marks.arrow(tip, dir);
            }
        }
        let [x0, y1] = marks.up(card.min);
        let [x1, y0] = marks.up(card.max);
        marks.out.push(SheetMark::Box { rect: [x0, y0, x1, y1], fill: Some(NOTE_FILL), stroke: Some((BLACK, 0.6)) });
        for (k, line) in lines.iter().enumerate() {
            let baseline = card.min + vec2(4.0, 3.0 + line_height * (k as f32 + 1.0) - 2.0);
            let at = marks.up(baseline);
            marks.out.push(SheetMark::Text { at, size: TEXT_SIZE, text: line.clone(), colour: BLACK });
        }
    }
}

/// A row of cells along the bottom: the part, the file, the units and the date.
fn title_block(out: &mut Vec<SheetMark>, rect: [f64; 4], part: &str, subject: &Subject<'_>) {
    out.push(SheetMark::Box { rect, fill: None, stroke: Some((BLACK, 1.0)) });
    let inch = (subject.units.mm_per_unit - 25.4).abs() < 1e-9;
    let units = if inch { "Inches" } else { "Millimetres" };
    let cells = [("PART", part, 0.46), ("FILE", subject.file, 0.28), ("UNITS", units, 0.12), ("DATE", subject.date.as_str(), 0.14)];
    let width = rect[2] - rect[0];
    let mut x = rect[0];
    for (k, (label, value, share)) in cells.iter().enumerate() {
        let cell = width * share;
        if k > 0 {
            out.push(SheetMark::Line { from: [x, rect[1]], to: [x, rect[3]], width: 0.8, colour: BLACK });
        }
        out.push(SheetMark::Text { at: [x + 6.0, rect[3] - 11.0], size: 6.5, text: (*label).to_owned(), colour: GREY });
        // The value, cut short to fit its cell.
        let size = if k == 0 { 13.0 } else { 10.0 };
        let mut text = (*value).to_owned();
        while !text.is_empty() && pdfcraft_fonts::helvetica_width(&text, size) > cell - 12.0 {
            text.pop();
        }
        out.push(SheetMark::Text { at: [x + 6.0, rect[1] + 10.0], size, text, colour: BLACK });
        x += cell;
    }
}

/// Today's date as `YYYY-MM-DD`.
pub(crate) fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    let (y, m, d) = pdfcraft_cos::civil_from_days(i64::try_from(days).unwrap_or(0));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Which of `layers` a sheet of `part` draws: its own, not hidden.
pub(crate) fn shown_layers<'a>(
    layers: &'a [pdfcraft_engine::Layer3d],
    part: &str,
    hidden: &std::collections::HashSet<usize>,
) -> Vec<&'a pdfcraft_engine::Layer3d> {
    layers.iter().enumerate().filter(|(k, l)| l.part == part && !hidden.contains(k)).map(|(_, l)| l).collect()
}
