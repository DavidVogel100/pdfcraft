//! The layers again as Acrobat reads them: one 3D view per layer in the 3D stream's `/VA`,
//! turned to the layer's view and with the other parts hidden, and its dimensions and notes as
//! 3D measurement dictionaries in the view's `/MA` (Adobe Supplement to ISO 32000, ExtensionLevel
//! 3, §9.5.6: `LD3` linear, `RD3` radial, `3DC` comment). As Acrobat writes its own, each
//! measurement is an object of its own tied (`/S`, `/ExData /M3DREF`) to a Projection annotation
//! on the page, which carries it in the comment list. What is written here is marked with
//! `/PCft3DView` and replaced on every change; the file's own views and comments stay.

use super::super::lookup::{annots, page_ref, set_annots};
use super::super::{AnnotError, Dict, Document, ObjRef, Object, PdfString};

/// Marks a view this module wrote.
const MARK: &[u8] = b"PCft3DView";

/// A view as Acrobat shows it: its name, camera, the model tree nodes it hides, and its
/// measurements. Everything in world coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardView3d {
    pub name: String,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    /// `Some(visible height)` for a parallel view, else perspective with `fov_y` degrees.
    pub ortho_height: Option<f64>,
    pub fov_y: f64,
    /// Model tree nodes hidden in this view, by name.
    pub hide: Vec<String>,
    pub measures: Vec<StandardMeasure3d>,
}

/// One measurement or note on a view.
#[derive(Debug, Clone, PartialEq)]
pub enum StandardMeasure3d {
    /// Between two points.
    Linear { from: [f64; 3], to: [f64; 3], text_at: [f64; 3], value: f64, units: String, places: u8, after: String, label: String },
    /// A round's radius or diameter: its centre and a point on it.
    Radial { centre: [f64; 3], on: [f64; 3], text_at: [f64; 3], value: f64, radius: bool, units: String, places: u8, after: String, label: String },
    /// A note pointing at `anchor`.
    Comment { anchor: [f64; 3], text_at: [f64; 3], text: String },
}

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().map(|&v| Object::Real(if v.is_finite() { v } else { 0.0 })).collect())
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l > 0.0 && l.is_finite()).then(|| a.map(|v| v / l))
}

/// The view's dictionary (its measurements to add as `/MA`), and the plane, across and up
/// directions its markup is written in: `/C2W` with z ahead, y up and x = z × y (as the 3D view
/// reader reads it), `/CO` the distance to the target.
fn view_dict(v: &StandardView3d) -> Option<(Dict, [[f64; 3]; 3])> {
    let ahead = unit(sub(v.target, v.eye))?;
    let right = unit(cross(ahead, v.up))?;
    let up = cross(right, ahead);
    let d = sub(v.target, v.eye);
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let mut c2w = Vec::with_capacity(12);
    for axis in [right, up, ahead, v.eye] {
        c2w.extend_from_slice(&axis);
    }
    let mut view = Dict::new();
    view.set(b"Type".to_vec(), Object::name("3DView"));
    view.set(b"XN".to_vec(), PdfString::text(&v.name));
    view.set(b"IN".to_vec(), PdfString::text(&v.name));
    view.set(b"MS".to_vec(), Object::name("M"));
    view.set(b"C2W".to_vec(), numbers(&c2w));
    view.set(b"CO".to_vec(), Object::Real(distance));
    let mut p = Dict::new();
    match v.ortho_height.filter(|h| *h > 0.0 && h.is_finite()) {
        Some(h) => {
            p.set(b"Subtype".to_vec(), Object::name("O"));
            p.set(b"OS".to_vec(), Object::Real(1.0 / h));
            p.set(b"OB".to_vec(), Object::name("H"));
        }
        None => {
            p.set(b"Subtype".to_vec(), Object::name("P"));
            p.set(b"FOV".to_vec(), Object::Real(v.fov_y));
            p.set(b"PS".to_vec(), Object::name("H"));
        }
    }
    view.set(b"P".to_vec(), Object::Dict(p));
    if !v.hide.is_empty() {
        let nodes = v
            .hide
            .iter()
            .map(|n| {
                let mut node = Dict::new();
                node.set(b"Type".to_vec(), Object::name("3DNode"));
                node.set(b"N".to_vec(), PdfString::text(n));
                node.set(b"V".to_vec(), Object::Bool(false));
                Object::Dict(node)
            })
            .collect();
        view.set(b"NA".to_vec(), Object::Array(nodes));
    }
    view.set(MARK.to_vec(), Object::Bool(true));
    // Text faces the camera: across is right, up is up; the markup lies square to the view.
    Some((view, [ahead.map(|v| -v), right, up]))
}

/// A colour as Acrobat writes a measurement's: with its colour space.
fn colour(c: [f64; 3]) -> Object {
    let mut a = vec![Object::name("DeviceRGB")];
    a.extend(c.iter().map(|&v| Object::Real(v)));
    Object::Array(a)
}

/// What a measurement says, for its comment.
fn said(m: &StandardMeasure3d) -> &str {
    match m {
        StandardMeasure3d::Linear { label, .. } | StandardMeasure3d::Radial { label, .. } => label,
        StandardMeasure3d::Comment { text, .. } => text,
    }
}

/// The Projection annotation carrying measurement `measure` on `page` in the comment list.
fn projection(page: ObjRef, measure: ObjRef, text: &str, k: usize) -> Dict {
    let mut ex = Dict::new();
    ex.set(b"Type".to_vec(), Object::name("ExData"));
    ex.set(b"Subtype".to_vec(), Object::name("3DM"));
    ex.set(b"M3DREF".to_vec(), Object::Ref(measure));
    let mut a = Dict::new();
    a.set(b"Type".to_vec(), Object::name("Annot"));
    a.set(b"Subtype".to_vec(), Object::name("Projection"));
    a.set(b"Rect".to_vec(), numbers(&[0.0; 4]));
    a.set(b"F".to_vec(), Object::Int(4));
    a.set(b"P".to_vec(), Object::Ref(page));
    a.set(b"Contents".to_vec(), PdfString::text(text));
    a.set(b"NM".to_vec(), PdfString::text(&format!("pdfcraft-3d-{}-{k}", measure.num)));
    a.set(b"T".to_vec(), PdfString::text("pdfcraft"));
    a.set(b"C".to_vec(), numbers(&[0.86, 0.2, 0.15]));
    a.set(b"ExData".to_vec(), Object::Dict(ex));
    a.set(MARK.to_vec(), Object::Bool(true));
    a
}

fn measure_dict(m: &StandardMeasure3d, plane: [f64; 3], across: [f64; 3], up: [f64; 3]) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("3DMeasure"));
    d.set(b"AP".to_vec(), numbers(&plane));
    d.set(b"TX".to_vec(), numbers(&across));
    d.set(b"TY".to_vec(), numbers(&up));
    d.set(b"TS".to_vec(), Object::Real(16.0));
    d.set(b"C".to_vec(), colour([0.0, 0.0, 0.0]));
    let value = |d: &mut Dict, value: f64, units: &str, places: u8, after: &str, label: &str| {
        d.set(b"V".to_vec(), Object::Real(value));
        d.set(b"U".to_vec(), PdfString::text(units));
        d.set(b"P".to_vec(), Object::Int(i64::from(places)));
        if !after.is_empty() {
            d.set(b"UT".to_vec(), PdfString::text(after));
        }
        d.set(b"TRL".to_vec(), PdfString::text(label));
    };
    match m {
        StandardMeasure3d::Linear { from, to, text_at, value: v, units, places, after, label } => {
            d.set(b"Subtype".to_vec(), Object::name("LD3"));
            d.set(b"A1".to_vec(), numbers(from));
            d.set(b"A2".to_vec(), numbers(to));
            d.set(b"TP".to_vec(), numbers(text_at));
            value(&mut d, *v, units, *places, after, label);
        }
        StandardMeasure3d::Radial { centre, on, text_at, value: v, radius, units, places, after, label } => {
            d.set(b"Subtype".to_vec(), Object::name("RD3"));
            d.set(b"A1".to_vec(), numbers(centre));
            d.set(b"A2".to_vec(), numbers(on));
            d.set(b"TP".to_vec(), numbers(text_at));
            d.set(b"SC".to_vec(), Object::Bool(false));
            d.set(b"R".to_vec(), Object::Bool(*radius));
            value(&mut d, *v, units, *places, after, label);
        }
        StandardMeasure3d::Comment { anchor, text_at, text } => {
            // Acrobat keeps a comment's text after the value (`/UT`), its box in characters and
            // lines (`/TB`).
            d.set(b"Subtype".to_vec(), Object::name("3DC"));
            d.set(b"A1".to_vec(), numbers(anchor));
            d.set(b"TP".to_vec(), numbers(text_at));
            d.set(b"UT".to_vec(), PdfString::text(text));
            d.set(b"TRL".to_vec(), PdfString::text(text.lines().next().unwrap_or_default()));
            let columns = text.lines().map(|l| l.chars().count()).max().unwrap_or(1).clamp(1, 60);
            let rows = text.lines().count().max(1);
            d.set(b"TB".to_vec(), Object::Array(vec![Object::Int(columns as i64), Object::Int(rows as i64)]));
        }
    }
    d
}

/// Whether the document carries signatures that a full rewrite would break.
fn signed(doc: &Document) -> bool {
    let Some(root) = doc.root() else { return false };
    let form = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").map(|f| doc.resolve(f)));
    form.as_deref().and_then(Object::as_dict).and_then(|f| f.get(b"SigFlags").and_then(|s| doc.resolve(s).as_int())).is_some_and(|f| f & 1 != 0)
}

/// Replaces the views written for the layers on the 3D annotation at `/Annots` position `index`
/// of 0-based `page` with `views`. The 3D stream's `/VA` is made an object of its own the first
/// time, so later changes leave the (large) stream alone; that first time the next save rewrites
/// the whole file rather than appending a copy of the stream, unless the file is signed.
///
/// # Errors
/// When there is no such page or annotation. A model with no 3D stream of its own to hold views
/// is left as it is.
pub fn set_standard_views(doc: &mut Document, page: usize, index: usize, views: &[StandardView3d]) -> Result<(), AnnotError> {
    let page_obj = page_ref(doc, page)?;
    let list = annots(doc, page_obj);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let annot = doc.dict(entry).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    // The 3D stream, directly or through a 3D reference dictionary; only an object of its own.
    let Some(dd) = annot.get(b"3DD") else { return Ok(()) };
    let Some(mut stream_ref) = dd.as_ref() else { return Ok(()) };
    if let Some(d) = doc.get(stream_ref).as_dict()
        && d.name(b"Type") == Some(b"3DRef")
    {
        let Some(r) = d.reference(b"3D") else { return Ok(()) };
        stream_ref = r;
    }
    let stream = doc.get(stream_ref);
    let Object::Stream(s) = stream.as_ref() else { return Ok(()) };
    let (va_ref, mut items) = match s.dict.get(b"VA") {
        Some(Object::Ref(r)) => (Some(*r), doc.get(*r).as_array().cloned().unwrap_or_default()),
        Some(Object::Array(a)) => (None, a.clone()),
        _ => (None, Vec::new()),
    };
    if va_ref.is_none() && views.is_empty() {
        return Ok(());
    }
    // The file's own views and comments stay; those written before go.
    let ours = |doc: &Document, o: &Object| doc.resolve(o).as_dict().is_some_and(|d| d.get(MARK).is_some());
    items.retain(|o| !ours(doc, o));
    let mut list: Vec<Object> = list.iter().filter(|o| !ours(doc, o)).cloned().collect();
    let had = annots(doc, page_obj).len();
    let mut k = 0;
    for v in views {
        let Some((mut d, [plane, across, up])) = view_dict(v) else { continue };
        let mut ma = Vec::new();
        for m in &v.measures {
            let measure = doc.add(Object::Dict(measure_dict(m, plane, across, up)));
            let comment = doc.add(Object::Dict(projection(page_obj, measure, said(m), k)));
            k += 1;
            doc.update_dict(measure, |d| d.set(b"S".to_vec(), Object::Ref(comment))).map_err(|e| AnnotError::Invalid(e.to_string()))?;
            list.push(Object::Ref(comment));
            ma.push(Object::Ref(measure));
        }
        d.set(b"MA".to_vec(), Object::Ref(doc.add(Object::Array(ma))));
        items.push(Object::Ref(doc.add(Object::Dict(d))));
    }
    if list.len() != had || k > 0 {
        set_annots(doc, page_obj, list).map_err(|e| AnnotError::Invalid(e.to_string()))?;
    }
    match va_ref {
        Some(r) => doc.set_existing(r, Object::Array(items)).map_err(|e| AnnotError::Invalid(e.to_string()))?,
        None => {
            let r = doc.add(Object::Array(items));
            let mut s = s.clone();
            s.dict.set(b"VA".to_vec(), Object::Ref(r));
            doc.set_existing(stream_ref, Object::Stream(s)).map_err(|e| AnnotError::Invalid(e.to_string()))?;
            if !signed(doc) {
                doc.require_full_save();
            }
        }
    }
    Ok(())
}
