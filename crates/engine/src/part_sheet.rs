//! A part sheet: one page with a picture of a 3D model's part and, drawn over it as vectors so
//! they print sharp, its dimensions, notes and a title block. Built from what the viewer worked
//! out (the picture and every mark in page coordinates); opened as a new document to print or
//! save.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Object, Stream};

use crate::{EditError, Session};

/// The picture of the part, `width × height` pixels of straight RGBA rows, drawn into `rect`
/// (`[x0, y0, x1, y1]`, page points from the bottom left).
#[derive(Clone, Debug, PartialEq)]
pub struct SheetPicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub rect: [f64; 4],
}

/// Something drawn over the picture, in page points from the bottom left.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetMark {
    /// A straight line `width` points wide.
    Line { from: [f64; 2], to: [f64; 2], width: f64, colour: [u8; 3] },
    /// A filled shape (an arrowhead).
    Fill { points: Vec<[f64; 2]>, colour: [u8; 3] },
    /// A box, filled and or outlined.
    Box { rect: [f64; 4], fill: Option<[u8; 3]>, stroke: Option<([u8; 3], f64)> },
    /// One line of Helvetica text, its baseline starting at `at`.
    Text { at: [f64; 2], size: f64, text: String, colour: [u8; 3] },
}

/// One page: its size in points, the picture, the marks over it, and the document's title.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSheet {
    pub page: (f64, f64),
    pub picture: SheetPicture,
    pub marks: Vec<SheetMark>,
    pub title: String,
}

/// The most pixels a sheet's picture may have.
const MAX_PICTURE_PIXELS: u64 = 64 * 1024 * 1024;

/// Text as Helvetica writes it: what WinAnsi has, the rest replaced by its nearest (`−` as `-`,
/// `≈` as `~`) or `?`.
pub fn sheet_text(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '−' | '–' => b'-',
            '≈' => b'~',
            c => pdfcraft_fonts::win_ansi_byte(c).unwrap_or(b'?'),
        })
        .collect()
}

fn number(out: &mut Vec<u8>, v: f64) {
    let v = if v.is_finite() { v } else { 0.0 };
    out.extend_from_slice(format!("{:.3} ", v).as_bytes());
}

fn colour(out: &mut Vec<u8>, c: [u8; 3], op: &str) {
    for k in c {
        number(out, f64::from(k) / 255.0);
    }
    out.extend_from_slice(op.as_bytes());
    out.push(b'\n');
}

/// The page's content: the picture, then each mark.
fn content(sheet: &PartSheet) -> Vec<u8> {
    let mut out = Vec::new();
    let [x0, y0, x1, y1] = sheet.picture.rect;
    out.extend_from_slice(b"q\n");
    for v in [x1 - x0, 0.0, 0.0, y1 - y0, x0, y0] {
        number(&mut out, v);
    }
    out.extend_from_slice(b"cm /Im0 Do Q\n");
    for mark in &sheet.marks {
        match mark {
            SheetMark::Line { from, to, width, colour: c } => {
                colour(&mut out, *c, "RG");
                number(&mut out, *width);
                out.extend_from_slice(b"w 1 J\n");
                number(&mut out, from[0]);
                number(&mut out, from[1]);
                out.extend_from_slice(b"m ");
                number(&mut out, to[0]);
                number(&mut out, to[1]);
                out.extend_from_slice(b"l S\n");
            }
            SheetMark::Fill { points, colour: c } => {
                let Some((first, rest)) = points.split_first() else { continue };
                colour(&mut out, *c, "rg");
                number(&mut out, first[0]);
                number(&mut out, first[1]);
                out.extend_from_slice(b"m ");
                for p in rest {
                    number(&mut out, p[0]);
                    number(&mut out, p[1]);
                    out.extend_from_slice(b"l ");
                }
                out.extend_from_slice(b"h f\n");
            }
            SheetMark::Box { rect, fill, stroke } => {
                let rect_op = |out: &mut Vec<u8>| {
                    for v in [rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1]] {
                        number(out, v);
                    }
                    out.extend_from_slice(b"re ");
                };
                if let Some(f) = fill {
                    colour(&mut out, *f, "rg");
                    rect_op(&mut out);
                    out.extend_from_slice(b"f\n");
                }
                if let Some((c, w)) = stroke {
                    colour(&mut out, *c, "RG");
                    number(&mut out, *w);
                    out.extend_from_slice(b"w ");
                    rect_op(&mut out);
                    out.extend_from_slice(b"S\n");
                }
            }
            SheetMark::Text { at, size, text, colour: c } => {
                colour(&mut out, *c, "rg");
                out.extend_from_slice(b"BT /F1 ");
                number(&mut out, *size);
                out.extend_from_slice(b"Tf ");
                number(&mut out, at[0]);
                number(&mut out, at[1]);
                out.extend_from_slice(b"Td (");
                for b in sheet_text(text) {
                    if matches!(b, b'(' | b')' | b'\\') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
                out.extend_from_slice(b") Tj ET\n");
            }
        }
    }
    out
}

/// The sheet as a PDF document.
///
/// # Errors
/// When the page or picture is out of range.
pub fn part_sheet(sheet: &PartSheet) -> Result<pdfcraft_cos::Document, String> {
    let p = &sheet.picture;
    let pixels = u64::from(p.width) * u64::from(p.height);
    if p.width == 0 || p.height == 0 || pixels > MAX_PICTURE_PIXELS || p.rgba.len() as u64 != pixels * 4 {
        return Err("the picture of the part is the wrong size".into());
    }
    let (w, h) = sheet.page;
    let mut doc = pdfcraft_create::from_contents(&[(w, h, Dict::new(), content(sheet))]).map_err(|e| e.to_string())?;
    // The picture, without its alpha (drawn on white).
    let rgb: Vec<u8> = p
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| {
            let a = u32::from(px[3]);
            // At most 255 × 255 + 255 × 255 over 255: a byte.
            [0, 1, 2].map(|k| u8::try_from((u32::from(px[k]) * a + 255 * (255 - a)) / 255).unwrap_or(u8::MAX))
        })
        .collect();
    let mut image = Dict::new();
    image.set(b"Type".to_vec(), Object::name("XObject"));
    image.set(b"Subtype".to_vec(), Object::name("Image"));
    image.set(b"Width".to_vec(), Object::Int(i64::from(p.width)));
    image.set(b"Height".to_vec(), Object::Int(i64::from(p.height)));
    image.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    image.set(b"Interpolate".to_vec(), Object::Bool(true));
    let image = doc.add(Object::Stream(Stream::flate(image, &rgb)));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let font = doc.add(Object::Dict(font));
    let mut xobjects = Dict::new();
    xobjects.set(b"Im0".to_vec(), Object::Ref(image));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(font));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    // The one page, through the page tree's kids (a fresh document keeps them inline).
    let page = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages")))
        .and_then(|p| doc.get(p).as_dict().and_then(|d| d.get(b"Kids").map(|k| doc.resolve(k))))
        .and_then(|k| k.as_array().and_then(|a| a.first()).and_then(Object::as_ref))
        .ok_or("the sheet has no page")?;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).map_err(|e| e.to_string())?;
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), pdfcraft_cos::PdfString::text(&sheet.title));
    let info = doc.add(Object::Dict(info));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    Ok(doc)
}

impl Session {
    /// A part sheet ([`part_sheet`]) as the bytes of a new PDF, checked to open.
    ///
    /// # Errors
    /// When the sheet can't be made or written.
    pub fn create_part_sheet(&self, sheet: &PartSheet) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = part_sheet(sheet).map_err(EditError::Invalid)?;
        self.write_new(&doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sheet_is_one_page_with_its_picture_and_marks() {
        let sheet = PartSheet {
            page: (792.0, 612.0),
            picture: SheetPicture { width: 2, height: 2, rgba: vec![255; 16], rect: [36.0, 36.0, 756.0, 576.0] },
            marks: vec![
                SheetMark::Line { from: [100.0, 100.0], to: [200.0, 100.0], width: 0.6, colour: [0, 0, 0] },
                SheetMark::Fill { points: vec![[200.0, 100.0], [192.0, 103.0], [192.0, 97.0]], colour: [0, 0, 0] },
                SheetMark::Box { rect: [500.0, 36.0, 756.0, 100.0], fill: Some([255, 255, 255]), stroke: Some(([0, 0, 0], 1.0)) },
                SheetMark::Text { at: [150.0, 104.0], size: 10.0, text: "Ø0.375 ±0.005 (x)".into(), colour: [0, 0, 0] },
            ],
            title: "Lid".into(),
        };
        let session = Session::new();
        let bytes = session.create_part_sheet(&sheet).unwrap();
        let doc = pdfcraft_cos::Document::open(bytes).unwrap();
        assert_eq!(pdfcraft_organize::page_count(&doc).unwrap(), 1);
        assert_eq!(sheet_text("Ø0.375 ±0.005 −0.002 ≈"), b"\xd80.375 \xb10.005 -0.002 ~");
        assert!(content(&sheet).windows(4).any(|w| w == br"\(x\"), "brackets escaped");
    }
}
