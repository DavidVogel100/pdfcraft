//! The 3D view on a page, driven like a user: a model is shown, and drags turn or move it.

use crate::PdfCraftApp;
use pdfcraft_prc_render::Camera;

/// A minimal PRC model: one triangle, with no assembly tree (the automation tests' fixture).
const TRIANGLE_PRC_HEX: &str = "505243c91f0000c91f00000100000002000000030000000400000000000000000000000000000000000000010000000500000006000000070000000800000000000000060000006b0000009a000000a3000000ab000000cd000000d5000000dd000000e500000000000000505243c91f0000c91f0000050000000600000007000000080000000000000000000000000000000000000000000000789c63000000010001789c030000000001789c9b71c087758da2422b43084fe3140e038713464cd759381a162800006ed70828789c030000000001789c030000000001789c030000000001";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// One page, 400 × 300, wholly covered by a 3D annotation holding `prc`.
fn pdf_with_model(prc: &[u8]) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /3D /Rect [0 0 400 300] /3DD 5 0 R >>".to_vec(),
    ];
    let mut stream = format!("<< /Type /3D /Subtype /PRC /Length {} >>\nstream\n", prc.len()).into_bytes();
    stream.extend_from_slice(prc);
    stream.extend_from_slice(b"\nendstream");
    objs.push(stream);
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
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

/// The app showing the model, and where on screen the middle of it is; at 60 frames a second,
/// so two clicks a frame apart make a double-click.
fn shown() -> (Harness, egui::Pos2) {
    let pdf = pdf_with_model(&unhex(TRIANGLE_PRC_HEX));
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, pdf.clone()).unwrap();
        app
    });
    for _ in 0..2000 {
        h.run_steps(2);
        let v = &h.state().views[0];
        if let (Some(r), Some(_)) = (v.page_screen_rect(0), v.view3d.camera(0, 0)) {
            return (h, r.center());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the model never showed");
}

/// A primary-button drag from `at` by `by`, in steps, with `modifiers` held.
fn drag(h: &mut Harness, at: egui::Pos2, by: egui::Vec2, modifiers: egui::Modifiers) {
    // The key goes down first, as a user presses Shift before the button.
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.run_steps(1);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.run_steps(1);
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(at + by * (k as f32 / 8.0)));
        h.run_steps(1);
    }
    let end = at + by;
    h.event(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    h.run_steps(1);
}

fn direction(c: &Camera) -> [f64; 3] {
    [0, 1, 2].map(|k| c.target[k] - c.eye[k])
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    let size = (0..3).map(|k| a[k].abs().max(b[k].abs())).fold(1e-12, f64::max);
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6 * size)
}

fn camera(h: &Harness) -> Camera {
    h.state().views[0].view3d.camera(0, 0).unwrap()
}

/// Zoomed in tight with the wheel, Shift+drag still moves the model, in every direction.
#[test]
fn a_shift_drag_moves_a_model_zoomed_in_tight() {
    let (mut h, at) = shown();
    for _ in 0..40 {
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 3.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(4);
    let zoomed = camera(&h);
    for by in [egui::vec2(80.0, 0.0), egui::vec2(-80.0, 0.0), egui::vec2(0.0, 80.0), egui::vec2(0.0, -80.0)] {
        let before = camera(&h);
        drag(&mut h, at, by, egui::Modifiers::SHIFT);
        assert_ne!(camera(&h).target, before.target, "Shift+drag by {by:?} moves the zoomed model ({zoomed:?})");
    }
}

#[test]
fn a_drag_turns_the_model_a_shift_drag_moves_it_and_a_double_click_goes_back() {
    let (mut h, at) = shown();
    let before = camera(&h);

    drag(&mut h, at, egui::vec2(60.0, 0.0), egui::Modifiers::SHIFT);
    let moved = camera(&h);
    assert!(!close(moved.target, before.target), "Shift+drag moves the model: {:?} -> {:?}", before.target, moved.target);
    assert!(close(direction(&moved), direction(&before)), "Shift+drag doesn't turn it");

    drag(&mut h, at, egui::vec2(60.0, 0.0), egui::Modifiers::NONE);
    let turned = camera(&h);
    assert!(close(turned.target, moved.target), "a plain drag doesn't move it");
    assert!(!close(direction(&turned), direction(&moved)), "a plain drag turns it");

    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for _ in 0..2 {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(3);
    assert_eq!(camera(&h), before, "a double-click goes back to the view it opened on");
}

/// A document with a 3D model offers its Model Tree; a model with no assembly tree says so there.
#[test]
fn a_document_with_a_model_opens_on_its_model_tree() {
    let (mut h, _) = shown();
    assert_eq!(h.state().right, Some(crate::RightPanel::Model3d));
    h.run_steps(2);
    use egui_kittest::kittest::Queryable as _;
    h.get_by_label("This model has no named parts.");
}
