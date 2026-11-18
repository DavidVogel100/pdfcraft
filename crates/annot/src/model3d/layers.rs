//! Dimension layers on a 3D model: what a user measured on one part (or sub-assembly), kept with
//! the 3D annotation so the file carries them. Stored in the annotation's private `/PCft3D`
//! dictionary (a second-class name, ISO 32000-2 Annex E), which other readers ignore. Points are
//! in the part's own coordinates, so every copy of the part shows them.

use super::super::lookup::{annots, page_ref, set_annots};
use super::super::{AnnotError, Dict, Document, Object, PdfString};

/// The key of pdfcraft's own entry on a 3D annotation.
const KEY: &[u8] = b"PCft3D";
/// The version of what is stored under [`KEY`].
const VERSION: i64 = 1;
/// The most layers, and dimensions on one layer, read back from a file.
pub const MAX_LAYERS: usize = 1000;
pub const MAX_DIMENSIONS: usize = 1000;

/// What a layer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerKind3d {
    #[default]
    Dimensions,
    Notes,
}

/// A saved camera, in the part's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerView3d {
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub up: [f64; 3],
    /// `Some(visible height)` for a parallel view, else perspective with [`Self::fov_y`].
    pub ortho_height: Option<f64>,
    pub fov_y: f64,
}

/// What a dimension measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimKind3d {
    /// Points: from, to.
    Distance,
    /// Points: centre, a point on the round.
    Diameter,
    /// Points: centre, a point on the round.
    Radius,
    /// Points: where it is shown.
    Angle,
    /// Points: where it is shown.
    ArcLength,
}

impl DimKind3d {
    fn name(self) -> &'static [u8] {
        match self {
            Self::Distance => b"Distance",
            Self::Diameter => b"Diameter",
            Self::Radius => b"Radius",
            Self::Angle => b"Angle",
            Self::ArcLength => b"ArcLength",
        }
    }

    fn of(name: &[u8]) -> Option<Self> {
        [Self::Distance, Self::Diameter, Self::Radius, Self::Angle, Self::ArcLength].into_iter().find(|k| k.name() == name)
    }
}

/// One kept dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct Dim3d {
    pub kind: DimKind3d,
    /// In model units (degrees for an angle).
    pub value: f64,
    /// The points it is drawn from, as [`DimKind3d`] lists them, in the part's coordinates.
    pub points: Vec<[f64; 3]>,
    /// Where its text sits, in the part's coordinates.
    pub text_at: [f64; 3],
    /// Tolerance above and below (model units), 0 for none.
    pub plus: f64,
    pub minus: f64,
    /// Decimal places shown.
    pub places: u8,
    /// Measured between surfaces of known shape, not between the points picked.
    pub exact: bool,
}

/// A layer of dimensions or notes on one part.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layer3d {
    /// The part (or sub-assembly) it belongs to, by name without the instance number.
    pub part: String,
    pub kind: LayerKind3d,
    pub name: String,
    pub view: Option<LayerView3d>,
    pub dims: Vec<Dim3d>,
}

/// The layers kept on the 3D annotation at `/Annots` position `index` on 0-based `page`; none
/// when it has none or they can't be read.
///
/// # Errors
/// When there is no such page or annotation.
pub fn model_3d_layers(doc: &Document, page: usize, index: usize) -> Result<Vec<Layer3d>, AnnotError> {
    let list = annots(doc, page_ref(doc, page)?);
    let entry = list.get(index).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let annot = doc.dict(entry).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let Some(store) = annot.get(KEY).map(|o| doc.resolve(o)) else { return Ok(Vec::new()) };
    let Some(store) = store.as_dict() else { return Ok(Vec::new()) };
    if store.int(b"V") != Some(VERSION) {
        return Ok(Vec::new());
    }
    let layers = store.get(b"Layers").map(|o| doc.resolve(o));
    let Some(layers) = layers.as_deref().and_then(Object::as_array) else { return Ok(Vec::new()) };
    Ok(layers.iter().take(MAX_LAYERS).filter_map(|l| read_layer(doc, &doc.resolve(l))).collect())
}

/// Replaces the layers kept on the 3D annotation at `/Annots` position `index` on 0-based
/// `page` (none removes the entry).
///
/// # Errors
/// When there is no such page or annotation, or it is not a 3D annotation.
pub fn set_model_3d_layers(doc: &mut Document, page: usize, index: usize, layers: &[Layer3d]) -> Result<(), AnnotError> {
    let p = page_ref(doc, page)?;
    let mut list = annots(doc, p);
    let entry = list.get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let is_3d = |d: &Dict| d.get(b"Subtype").is_some_and(|s| s.as_name() == Some(b"3D"));
    let r = match entry {
        Object::Ref(r) if doc.get(r).as_dict().is_some_and(is_3d) => r,
        Object::Dict(d) if is_3d(&d) => {
            let r = doc.add(Object::Dict(d));
            if let Some(slot) = list.get_mut(index) {
                *slot = Object::Ref(r);
            }
            set_annots(doc, p, list).map_err(|e| AnnotError::Invalid(e.to_string()))?;
            r
        }
        _ => return Err(AnnotError::Invalid(format!("annotation {} on page {} is not a 3D model", index + 1, page + 1))),
    };
    let store = (!layers.is_empty()).then(|| {
        let mut d = Dict::new();
        d.set(b"V".to_vec(), Object::Int(VERSION));
        d.set(b"Layers".to_vec(), Object::Array(layers.iter().map(write_layer).collect()));
        Object::Dict(d)
    });
    doc.update_dict(r, |d| match store {
        Some(s) => d.set(KEY.to_vec(), s),
        None => {
            d.remove(KEY);
        }
    })
    .map_err(|e| AnnotError::Invalid(e.to_string()))
}

fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().map(|&v| Object::Real(v)).collect())
}

fn write_layer(l: &Layer3d) -> Object {
    let mut d = Dict::new();
    d.set(b"Part".to_vec(), PdfString::text(&l.part));
    d.set(b"Name".to_vec(), PdfString::text(&l.name));
    d.set(b"Kind".to_vec(), Object::name(if l.kind == LayerKind3d::Notes { "Notes" } else { "Dimensions" }));
    if let Some(v) = &l.view {
        let mut view = Dict::new();
        view.set(b"Eye".to_vec(), numbers(&v.eye));
        view.set(b"Target".to_vec(), numbers(&v.target));
        view.set(b"Up".to_vec(), numbers(&v.up));
        match v.ortho_height {
            Some(h) => view.set(b"Ortho".to_vec(), Object::Real(h)),
            None => view.set(b"FOV".to_vec(), Object::Real(v.fov_y)),
        }
        d.set(b"View".to_vec(), Object::Dict(view));
    }
    let dims = l
        .dims
        .iter()
        .map(|m| {
            let mut x = Dict::new();
            x.set(b"Type".to_vec(), Object::Name(m.kind.name().to_vec()));
            x.set(b"Value".to_vec(), Object::Real(m.value));
            x.set(b"P".to_vec(), numbers(&m.points.concat()));
            x.set(b"Text".to_vec(), numbers(&m.text_at));
            x.set(b"Plus".to_vec(), Object::Real(m.plus));
            x.set(b"Minus".to_vec(), Object::Real(m.minus));
            x.set(b"Places".to_vec(), Object::Int(i64::from(m.places)));
            x.set(b"Exact".to_vec(), Object::Bool(m.exact));
            Object::Dict(x)
        })
        .collect();
    d.set(b"Dims".to_vec(), Object::Array(dims));
    Object::Dict(d)
}

/// The finite numbers of an array, or `None` when any is missing or not finite.
fn read_numbers(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|v| doc.resolve(v).as_f64().filter(|x| x.is_finite())).collect()
}

fn point(doc: &Document, o: Option<&Object>) -> Option<[f64; 3]> {
    match read_numbers(doc, o)?.as_slice() {
        &[x, y, z] => Some([x, y, z]),
        _ => None,
    }
}

fn text(doc: &Document, o: Option<&Object>) -> Option<String> {
    doc.resolve(o?).as_string().map(PdfString::to_text)
}

fn real(doc: &Document, d: &Dict, key: &[u8]) -> Option<f64> {
    d.get(key).and_then(|o| doc.resolve(o).as_f64()).filter(|v| v.is_finite())
}

fn read_layer(doc: &Document, o: &Object) -> Option<Layer3d> {
    let d = o.as_dict()?;
    let kind = if d.name(b"Kind") == Some(b"Notes") { LayerKind3d::Notes } else { LayerKind3d::Dimensions };
    let view = d.get(b"View").map(|v| doc.resolve(v)).and_then(|v| {
        let v = v.as_dict()?;
        Some(LayerView3d {
            eye: point(doc, v.get(b"Eye"))?,
            target: point(doc, v.get(b"Target"))?,
            up: point(doc, v.get(b"Up"))?,
            ortho_height: real(doc, v, b"Ortho").filter(|h| *h > 0.0),
            fov_y: real(doc, v, b"FOV").filter(|f| *f > 0.0 && *f < 180.0).unwrap_or(30.0),
        })
    });
    let dims = d.get(b"Dims").map(|x| doc.resolve(x));
    let dims = dims
        .as_deref()
        .and_then(Object::as_array)
        .map(|list| {
            list.iter()
                .take(MAX_DIMENSIONS)
                .filter_map(|m| {
                    let m = doc.resolve(m);
                    let m = m.as_dict()?;
                    let kind = DimKind3d::of(m.name(b"Type")?)?;
                    let flat = read_numbers(doc, m.get(b"P"))?;
                    let (points, rest) = flat.as_chunks::<3>();
                    if !rest.is_empty() || points.is_empty() || points.len() > 8 {
                        return None;
                    }
                    Some(Dim3d {
                        kind,
                        value: real(doc, m, b"Value")?,
                        points: points.to_vec(),
                        text_at: point(doc, m.get(b"Text"))?,
                        plus: real(doc, m, b"Plus").unwrap_or(0.0).abs(),
                        minus: real(doc, m, b"Minus").unwrap_or(0.0).abs(),
                        places: m.int(b"Places").and_then(|p| u8::try_from(p.clamp(0, 6)).ok()).unwrap_or(3),
                        exact: !matches!(m.get(b"Exact").map(|e| doc.resolve(e)).as_deref(), Some(Object::Bool(false))),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Layer3d { part: text(doc, d.get(b"Part"))?, kind, name: text(doc, d.get(b"Name")).unwrap_or_default(), view, dims })
}
