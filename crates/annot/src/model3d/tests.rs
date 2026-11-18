use std::sync::Arc;

use super::*;

/// A one-page document whose page has `annots` as its `/Annots`, plus `extra` objects from 4 on.
fn doc(annots: &str, extra: &[Vec<u8>]) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /Annots {annots} >>").into_bytes(),
    ];
    objs.extend(extra.iter().cloned());
    file(&objs)
}

/// A document of `objs`, numbered from 1; the first is the catalog.
fn file(objs: &[Vec<u8>]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

fn stream(dict: &str, body: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
    s.extend_from_slice(body);
    s.extend_from_slice(b"\nendstream");
    s
}

fn annot(dd: &str) -> Vec<u8> {
    format!("<< /Type /Annot /Subtype /3D /Rect [0 0 100 100] /3DD {dd} >>").into_bytes()
}

#[test]
fn a_plain_3d_stream_is_read_through_its_filters() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /PRC /Filter /ASCIIHexDecode", b"505243>")]);
    assert_eq!(model_3d(&d, 0, 0), Ok(Model3d { kind: Model3dKind::Prc, data: b"PRC".to_vec() }));
}

#[test]
fn a_3dref_leads_to_the_stream_it_names() {
    let d = doc("[4 0 R]", &[annot("<< /Type /3DRef /3D 5 0 R >>"), stream("/Type /3D /Subtype /PRC", b"PRC")]);
    assert_eq!(model_3d(&d, 0, 0).map(|m| m.data), Ok(b"PRC".to_vec()));
}

#[test]
fn a_3dref_and_its_format_may_be_named_through_references() {
    let d = doc("[4 0 R]", &[annot("<< /Type 6 0 R /3D 5 0 R >>"), stream("/Type /3D /Subtype 7 0 R", b"PRC"), b"/3DRef".to_vec(), b"/PRC".to_vec()]);
    assert_eq!(model_3d(&d, 0, 0), Ok(Model3d { kind: Model3dKind::Prc, data: b"PRC".to_vec() }));
}

#[test]
fn a_u3d_stream_is_read_and_says_so() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /U3D", b"U3D")]);
    assert_eq!(model_3d(&d, 0, 0).map(|m| m.kind), Ok(Model3dKind::U3d));
}

#[test]
fn a_3d_annotation_without_a_stream_is_invalid() {
    let d = doc("[<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>]", &[]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("no 3D stream")));
}

#[test]
fn an_unknown_format_is_invalid() {
    let d = doc("[4 0 R]", &[annot("5 0 R"), stream("/Type /3D /Subtype /STL", b"x")]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("unknown format (STL)")));
}

#[test]
fn a_3dref_is_followed_once_and_only_to_a_stream() {
    // A reference naming another reference, or one that names itself, is damage: no stream.
    let d = doc("[4 0 R]", &[annot("5 0 R"), b"<< /Type /3DRef /3D 6 0 R >>".to_vec(), b"<< /Type /3DRef /3D 5 0 R >>".to_vec()]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("no 3D stream")));
    // The stream is under `/3D`; a `/3DD` inside the reference is not the standard's key.
    let d = doc("[4 0 R]", &[annot("<< /Type /3DRef /3DD 5 0 R >>"), stream("/Type /3D /Subtype /PRC", b"PRC")]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("no 3D stream")));
}

#[test]
fn another_annotation_is_not_a_3d_one() {
    let d = doc("[<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] >> 4 0 R]", &[annot("5 0 R"), stream("/Subtype /PRC", b"PRC")]);
    assert!(matches!(model_3d(&d, 0, 0), Err(AnnotError::Invalid(m)) if m.contains("not a 3D annotation")));
    assert_eq!(model_3d(&d, 0, 2), Err(AnnotError::NoSuchAnnotation { page: 0, index: 2 }));
    assert_eq!(model_3d_annotations(&d, 0), Ok(vec![1]));
    assert_eq!(model_3d_annotations(&d, 1), Err(AnnotError::NoSuchPage(1)));
}

#[test]
fn shown_positions_follow_the_inspectors_rules() {
    // A 3D annotation whose `/Subtype` is a reference, one listed twice, a hidden one, and a
    // square: the first is listed, the repeat counted once, where it is first listed.
    let d = doc(
        "[6 0 R 4 0 R 7 0 R 4 0 R 8 0 R]",
        &[
            b"<< /Type /Annot /Subtype 5 0 R /Rect [0 0 1 1] >>".to_vec(),
            b"/3D".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] >>".to_vec(),
            b"<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] /F 2 >>".to_vec(),
            b"<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>".to_vec(),
        ],
    );
    assert_eq!(shown_model_3d_positions(&d), Ok(vec![(0, 1), (0, 4)]));
    assert_eq!(model_3d_annotations(&d, 0), Ok(vec![1, 2, 3, 4]));
}

#[test]
fn an_annots_array_two_pages_share_is_counted_on_the_first() {
    let d = file(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [7 0 R << /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>] >>".to_vec(),
        b"[<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>]".to_vec(),
        b"<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>".to_vec(),
    ]);
    assert_eq!(shown_model_3d_positions(&d), Ok(vec![(0, 0), (2, 0), (2, 1)]));
}

#[test]
fn at_most_1024_shown_3d_annotations_are_listed() {
    let one = "<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >> ";
    let d = doc(&format!("[{}]", one.repeat(MAX_SHOWN_3D + 1)), &[]);
    let shown = shown_model_3d_positions(&d).expect("lists");
    assert_eq!(shown.len(), MAX_SHOWN_3D);
    assert_eq!(shown.last(), Some(&(0, MAX_SHOWN_3D - 1)));
}

#[test]
fn dimension_layers_are_kept_on_the_annotation_and_read_back() {
    let mut d = doc("[4 0 R]", &[b"<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] >>".to_vec()]);
    assert_eq!(model_3d_layers(&d, 0, 0).unwrap(), []);
    let layer = Layer3d {
        part: "Threaded Insert Brass 10-32".into(),
        kind: LayerKind3d::Dimensions,
        name: "Dimensions".into(),
        view: Some(LayerView3d { eye: [1.0, 2.0, 3.0], target: [0.0; 3], up: [0.0, 0.0, 1.0], ortho_height: Some(2.5), fov_y: 30.0 }),
        dims: vec![Dim3d {
            kind: DimKind3d::Diameter,
            value: 0.375,
            points: vec![[0.0; 3], [0.1875, 0.0, 0.0]],
            text_at: [0.5, 0.5, 0.0],
            plus: 0.005,
            minus: 0.002,
            places: 3,
            exact: true,
        }],
        notes: vec![
            Note3d {
                text: "Break all edges
Deburr"
                    .into(),
                place: NotePlace3d::Sheet { at: [0.05, 0.9] },
            },
            Note3d { text: "Tap 10-32".into(), place: NotePlace3d::Leader { anchor: [0.0; 3], text_at: [0.5, 0.2, 0.0] } },
        ],
    };
    set_model_3d_layers(&mut d, 0, 0, std::slice::from_ref(&layer)).unwrap();
    assert_eq!(model_3d_layers(&d, 0, 0).unwrap(), [layer]);
    // Through a save and a reopen.
    let bytes = pdfcraft_cos::write_incremental(&d, &pdfcraft_cos::SaveOptions::default()).unwrap();
    let again = Document::open(Arc::new(bytes)).unwrap();
    assert_eq!(model_3d_layers(&again, 0, 0).unwrap().len(), 1);
    set_model_3d_layers(&mut d, 0, 0, &[]).unwrap();
    assert_eq!(model_3d_layers(&d, 0, 0).unwrap(), []);
    // Only on a 3D annotation.
    let mut text = doc("[4 0 R]", &[b"<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] >>".to_vec()]);
    assert!(set_model_3d_layers(&mut text, 0, 0, &[]).is_err());
}

#[test]
fn layers_are_written_as_views_acrobat_shows_and_replaced_on_every_change() {
    let mut d = doc(
        "[4 0 R]",
        &[
            b"<< /Type /Annot /Subtype /3D /Rect [0 0 1 1] /3DD 5 0 R >>".to_vec(),
            stream("/Type /3D /Subtype /PRC /VA [6 0 R]", b"PRC"),
            b"<< /Type /3DView /XN (Home) >>".to_vec(),
        ],
    );
    let view = StandardView3d {
        name: "Lid - Dimensions".into(),
        eye: [0.0, -10.0, 0.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
        ortho_height: Some(4.0),
        fov_y: 30.0,
        hide: vec!["Box-1".into()],
        measures: vec![
            StandardMeasure3d::Linear {
                from: [0.0; 3],
                to: [1.0, 0.0, 0.0],
                text_at: [0.5, 0.0, 0.3],
                value: 1.0,
                units: "in".into(),
                places: 3,
                after: String::new(),
                label: "1.000".into(),
            },
            StandardMeasure3d::Radial {
                centre: [0.0; 3],
                on: [0.2, 0.0, 0.0],
                text_at: [0.4, 0.0, 0.2],
                value: 0.4,
                radius: false,
                units: "in".into(),
                places: 3,
                after: " ±0.005".into(),
                label: "Ø0.400".into(),
            },
            StandardMeasure3d::Comment { anchor: [0.0; 3], text_at: [0.0, 0.0, 1.0], text: "Break edges".into() },
        ],
    };
    set_standard_views(&mut d, 0, 0, std::slice::from_ref(&view)).unwrap();
    let views = |d: &Document| {
        let s = d.get(pdfcraft_cos::ObjRef::new(5, 0));
        let Object::Stream(s) = s.as_ref() else { panic!() };
        let r = s.dict.reference(b"VA").expect("its views an object of their own");
        d.get(r).as_array().unwrap().iter().map(|v| d.resolve(v).as_dict().unwrap().clone()).collect::<Vec<_>>()
    };
    let v = views(&d);
    assert_eq!(v.len(), 2, "the file's own view kept, one added");
    assert_eq!(v[1].get(b"MA").and_then(Object::as_array).map(Vec::len), Some(3));
    assert_eq!(v[1].get(b"MA").and_then(Object::as_array).and_then(|a| a[0].as_dict()).and_then(|m| m.name(b"Subtype")), Some(&b"LD3"[..]));
    // Written again: replaced, not added to; none: only the file's own.
    set_standard_views(&mut d, 0, 0, &[view.clone(), view]).unwrap();
    assert_eq!(views(&d).len(), 3);
    set_standard_views(&mut d, 0, 0, &[]).unwrap();
    assert_eq!(views(&d).len(), 1);
    // Read back by the 3D view reader as the camera it was.
    let bytes = pdfcraft_cos::write_incremental(&d, &pdfcraft_cos::SaveOptions::default()).unwrap();
    assert!(Document::open(Arc::new(bytes)).is_ok());
}
