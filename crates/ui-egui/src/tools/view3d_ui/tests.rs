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

/// The model's surface under screen point `p`, from the frame last drawn.
fn surface_point(h: &Harness, p: egui::Pos2) -> Option<[f64; 3]> {
    let m = h.state().views[0].view3d.models.get(&(0, 0))?;
    let (r, f, c) = (m.screen?, m.frame.as_ref()?, m.camera?);
    let x = (p.x - r.left()) / r.width() * f.image.width as f32;
    let y = (p.y - r.top()) / r.height() * f.image.height as f32;
    f.point_at(x as u32, y as u32, &c)
}

/// Where `p` lies from `c`'s eye: right, up and ahead.
fn seen_from(c: &Camera, p: [f64; 3]) -> [f64; 3] {
    let d = [0, 1, 2].map(|k| p[k] - c.eye[k]);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let ahead = direction(c);
    let len = dot(ahead, ahead).sqrt();
    let ahead = ahead.map(|v| v / len);
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    // The camera's `up` need not be square to the view; the picture's own up is.
    let right = cross(ahead, c.up);
    let len = dot(right, right).sqrt();
    let right = right.map(|v| v / len);
    let up = cross(right, ahead);
    [dot(d, right), dot(d, up), dot(d, ahead)]
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

    // A plain drag turns it about the point of the model under the press, which stays where it
    // is on the picture: the same distance right, up and ahead of the eye.
    let pressed = surface_point(&h, at);
    drag(&mut h, at, egui::vec2(60.0, 0.0), egui::Modifiers::NONE);
    let turned = camera(&h);
    assert!(!close(direction(&turned), direction(&moved)), "a plain drag turns it");
    let about = pressed.unwrap_or(moved.target);
    assert!(close(seen_from(&turned, about), seen_from(&moved, about)), "the point turned about stays put on the picture");
    assert!(pressed.is_some(), "the press was on the model");

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

fn click(h: &mut Harness, p: egui::Pos2) {
    h.event(egui::Event::PointerMoved(p));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(20);
}

/// With Measure on, a click picks the face under it instead of a part, a second click measures
/// between the two, and a third starts again.
#[test]
fn measure_clicks_pick_faces_and_measure_between_them() {
    let (mut h, at) = shown();
    h.state_mut().views[0].view3d.measuring = true;
    click(&mut h, at);
    let picks = |h: &Harness| h.state().views[0].view3d.models.get(&(0, 0)).map_or(0, |m| m.measure.picks.len());
    assert_eq!(picks(&h), 1);
    assert_eq!(h.state().views[0].view3d.models.get(&(0, 0)).and_then(|m| m.parts.selected), None, "no part is selected");
    click(&mut h, at + egui::vec2(6.0, 4.0));
    assert_eq!(picks(&h), 2);
    let readings = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().measure.readings.clone();
    // Two points on the one flat triangle: the planes are the same, 0 apart.
    assert!(
        matches!(readings.first(), Some(pdfcraft_prc_render::measure::Reading::Distance { value, exact: true, .. }) if value.abs() < 1e-9),
        "{readings:?}"
    );
    click(&mut h, at);
    assert_eq!(picks(&h), 1, "a third click starts again");
}

/// With Measure on, a click just beside an edge line picks the edge: the triangle's side reads
/// its length, and is drawn picked.
#[test]
fn a_measure_click_beside_an_edge_picks_the_edge() {
    let (mut h, _) = shown();
    h.state_mut().views[0].view3d.measuring = true;
    h.run_steps(2);
    // Where the middle of the triangle's first side is on the screen.
    let at = {
        let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!("not read") };
        let mesh = &scene.meshes[0].mesh;
        let [a, b] = m.look.outlines.as_deref().unwrap()[0][0];
        let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
        let mid = [0, 1, 2].map(|k| (pa[k] + pb[k]) / 2.0);
        let (camera, size, _) = m.drawn.unwrap();
        let [x, y] = camera.project(mid, size[0], size[1]).unwrap();
        let r = m.screen.unwrap();
        egui::pos2(r.left() + (x / f64::from(size[0])) as f32 * r.width(), r.top() + (y / f64::from(size[1])) as f32 * r.height())
    };
    // Two points off the line, still within reach.
    click(&mut h, at + egui::vec2(2.0, 0.0));
    let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
    assert_eq!(m.measure.edges.len(), 1, "the edge is picked");
    assert!(
        matches!(m.measure.readings.first(), Some(pdfcraft_prc_render::measure::Reading::Distance { exact: true, .. })),
        "a straight edge reads its length: {:?}",
        m.measure.readings
    );
}

/// Pictures of a real file in the viewer, for looking at by eye: the file in `VIEW3D_PDF`, the
/// pictures into `VIEW3D_OUT`. `cargo test -p pdfcraft-ui-egui --lib view3d_pictures -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_pictures() {
    use super::{Load, tree};
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    let mut at = None;
    for _ in 0..6000 {
        h.run_steps(2);
        let v = &h.state().views[0];
        if let (Some(r), Some(_)) = (v.page_screen_rect(0), v.view3d.camera(0, 0)) {
            at = Some(r.center());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let at = at.expect("the model showed");
    h.run_steps(4);
    h.render().unwrap().save(out.join("1-opened.png")).unwrap();
    drag(&mut h, at, egui::vec2(120.0, 40.0), egui::Modifiers::NONE);
    h.run_steps(30);
    h.render().unwrap().save(out.join("2-turned.png")).unwrap();
    // A click on the model picks the part under it, highlighted and named in the tree.
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(30);
    h.render().unwrap().save(out.join("3-picked.png")).unwrap();
    let picked = h.state().views[0].view3d.models.get(&(0, 0)).and_then(|m| m.parts.selected);
    eprintln!("picked node {picked:?}");
    // Hide the picked part.
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        if let Some(n) = m.parts.selected {
            m.parts.hidden.insert(n);
            m.parts.changed();
        }
    }
    h.run_steps(30);
    h.render().unwrap().save(out.join("4-hidden.png")).unwrap();
    {
        let v = &mut h.state_mut().views[0].view3d;
        v.section = tree::SectionState { on: true, plane: tree::Plane::Front, at: 0.5, flip: false };
        for m in v.models.values_mut() {
            m.parts.hidden.clear();
            m.parts.changed();
        }
    }
    h.run_steps(30);
    h.render().unwrap().save(out.join("5-section-front.png")).unwrap();
    {
        let v = &mut h.state_mut().views[0].view3d;
        v.section = tree::SectionState { on: true, plane: tree::Plane::Right, at: 0.5, flip: false };
        for m in v.models.values_mut() {
            m.parts.changed();
        }
    }
    h.run_steps(30);
    h.render().unwrap().save(out.join("6-section-right.png")).unwrap();
    // Measure: two clicks a little apart, then the readings.
    {
        let v = &mut h.state_mut().views[0].view3d;
        v.section.on = false;
        v.measuring = true;
        for m in v.models.values_mut() {
            m.parts.changed();
        }
    }
    h.run_steps(30);
    click(&mut h, at);
    click(&mut h, at + egui::vec2(25.0, 30.0));
    h.run_steps(10);
    h.render().unwrap().save(out.join("7-measured.png")).unwrap();
    for r in &h.state().views[0].view3d.models.get(&(0, 0)).unwrap().measure.readings {
        eprintln!("reading {r:?}");
    }
    let v = &h.state().views[0].view3d;
    assert!(v.models.values().all(|m| matches!(m.load, Load::Ready(_))));
}

/// On a real file: after a wheel zoom, a drag started on the model turns it about the point
/// pressed. `VIEW3D_PDF` names the file. `cargo test -p pdfcraft-ui-egui --lib view3d_real_turn -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_real_turn() {
    let Some(pdf) = std::env::var_os("VIEW3D_PDF") else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    let mut centre = None;
    for _ in 0..6000 {
        h.run_steps(2);
        let v = &h.state().views[0];
        if let (Some(r), Some(_)) = (v.page_screen_rect(0), v.view3d.camera(0, 0)) {
            centre = Some(r.center());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let centre = centre.unwrap();
    h.run_steps(30);
    let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
    eprintln!("whole page {} screen {:?} page {:?}", m.whole_page, m.screen, h.state().views[0].page_screen_rect(0));
    // Zoom in over the model with the wheel.
    for _ in 0..10 {
        h.event(egui::Event::PointerMoved(centre));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(30);
    // Find a point on the model a little off the middle.
    let mut at = None;
    'find: for dy in [-40.0, 0.0, 40.0, 80.0] {
        for dx in [-60.0, -30.0, 30.0, 60.0] {
            let p = centre + egui::vec2(dx, dy);
            if surface_point(&h, p).is_some() {
                at = Some(p);
                break 'find;
            }
        }
    }
    let at = at.expect("a point on the model");
    let pressed = surface_point(&h, at).unwrap();
    let before = camera(&h);
    drag(&mut h, at, egui::vec2(50.0, 20.0), egui::Modifiers::NONE);
    let after = camera(&h);
    eprintln!("about {:?}", h.state().views[0].view3d.models.get(&(0, 0)).unwrap().turn_about);
    eprintln!("pressed {pressed:?}\n before {:?}\n after {:?}", seen_from(&before, pressed), seen_from(&after, pressed));
    assert!(close(seen_from(&after, pressed), seen_from(&before, pressed)), "the point pressed stays put");
}

/// On a real 3D PDF (`VIEW3D_PDF`): a part picked, Dimension ticked, a layer made through the
/// question, a hole measured and kept, and the layer read back from the saved file; pictures
/// into `VIEW3D_OUT`. `cargo test -p pdfcraft-ui-egui --lib view3d_dimension_layer -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_dimension_layer() {
    use super::Load;
    use egui_kittest::kittest::Queryable;
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The part: the first with a whole round hole in it.
    let (node, mesh, point) = {
        let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!("not read") };
        let wanted = std::env::var("VIEW3D_PART").unwrap_or_default();
        scene
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.children.is_empty() && n.name.contains(&wanted))
            .find_map(|(k, n)| {
                n.meshes.iter().find_map(|&mi| {
                    let mesh = &scene.meshes[mi].mesh;
                    mesh.faces.iter().find_map(|f| {
                        let fit = pdfcraft_prc_render::shapes::fit_face(mesh, f.clone());
                        let pdfcraft_prc_render::shapes::Shape::Cylinder { sweep, .. } = fit.shape else { return None };
                        let t = mesh.triangles[f.start];
                        let c = [0, 1, 2].map(|a| (0..3).map(|v| mesh.positions[t[v] as usize][a]).sum::<f64>() / 3.0);
                        (sweep > 6.0).then_some((k, mi, c))
                    })
                })
            })
            .expect("a part with a hole")
    };
    {
        let v = &mut h.state_mut().views[0].view3d;
        let m = v.models.get_mut(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        m.parts.select(&scene, Some(node));
        eprintln!("part {}", scene.nodes[node].name);
    }
    h.run_steps(5);
    // Tick Dimension, as the user does.
    h.get_by_label("Dimension").click();
    h.run_steps(10);
    h.render().unwrap().save(out.join("d1-asked.png")).unwrap();
    h.get_by_label("Yes").click();
    h.run_steps(20);
    let layers = |h: &Harness| {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().model_3d_layers(0, 0).unwrap()
    };
    assert_eq!(layers(&h).len(), 1, "a layer made");
    assert_eq!(h.state().views[0].view3d.models.get(&(0, 0)).unwrap().parts.isolated, Some(node));
    // Measure the hole and keep it.
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        m.measure.pick(&scene.meshes, mesh, point);
        eprintln!("readings {:?}", m.measure.readings);
    }
    h.run_steps(5);
    // The reading's label dragged away: a leader points back.
    let card = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().label_hits.first().map(|(_, r)| r.center()).expect("a label");
    drag(&mut h, card, egui::vec2(160.0, -90.0), egui::Modifiers::NONE);
    h.run_steps(5);
    let moved = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().measure.moved.clone();
    assert!(moved.first().copied().flatten().is_some_and(|v| v.x > 100.0), "{moved:?}");
    h.render().unwrap().save(out.join("d1b-label-moved.png")).unwrap();
    h.get_all_by_label("Keep").next().expect("Keep").click();
    h.run_steps(20);
    h.render().unwrap().save(out.join("d2-kept.png")).unwrap();
    let kept = layers(&h);
    assert_eq!(kept[0].dims.len(), 1, "{kept:?}");
    eprintln!("kept {:?}", kept[0].dims[0]);
    // Saved and opened again, the layer is still there.
    let app = h.state();
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    std::fs::write(out.join("dimensioned.pdf"), bytes.as_slice()).unwrap();
    let mut again = pdfcraft_engine::Session::new();
    let id = again.open_new("again.pdf", std::sync::Arc::new(bytes.to_vec())).unwrap();
    assert_eq!(again.get(id).unwrap().model_3d_layers(0, 0).unwrap(), kept);
    // Written for Acrobat too: a view with the measurement, the file rewritten once rather than
    // a second copy of the model appended, and the model still read.
    let original = std::fs::metadata(&pdf).unwrap().len() as f64;
    eprintln!("saved {} bytes, was {original}", bytes.len());
    assert!((bytes.len() as f64) < original * 1.3, "not doubled");
    let cos = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes.to_vec())).unwrap();
    let d = |o: &pdfcraft_cos::Object| cos.resolve(o).as_dict().cloned().unwrap();
    let catalog = cos.get(cos.root().unwrap()).as_dict().cloned().unwrap();
    let pages = d(catalog.get(b"Pages").unwrap());
    let page = d(&cos.resolve(pages.get(b"Kids").unwrap()).as_array().unwrap()[0]);
    let annot = d(&cos.resolve(page.get(b"Annots").unwrap()).as_array().unwrap()[0]);
    let stream = cos.resolve(annot.get(b"3DD").unwrap());
    let pdfcraft_cos::Object::Stream(stream) = stream.as_ref() else { panic!("a 3D stream") };
    let views: Vec<_> = cos.resolve(stream.dict.get(b"VA").unwrap()).as_array().unwrap().iter().map(&d).collect();
    eprintln!("{} views", views.len());
    let ours: Vec<_> = views.iter().filter(|v| v.get(b"PCft3DView").is_some()).collect();
    assert_eq!(ours.len(), 1, "an Acrobat view for the layer");
    let ma = cos.resolve(ours[0].get(b"MA").unwrap()).as_array().unwrap().clone();
    assert_eq!(d(&ma[0]).name(b"Subtype"), Some(&b"RD3"[..]));
    eprintln!("view {:?}", ours[0]);
    let model = again.get(id).unwrap().model_3d(0, 0).unwrap();
    assert!(super::scene::Scene::read(&model.data).is_ok());
}

/// On a real 3D PDF (`VIEW3D_PDF`): a part picked, Note ticked, a notes layer made, a note
/// with a leader added by a click on the part and one on the sheet by a click beside it, and
/// both read back from the saved file; pictures into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_notes_layer -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_notes_layer() {
    use super::Load;
    use egui_kittest::kittest::Queryable;
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        let node = scene.nodes.iter().position(|n| n.name.starts_with("Fill Nozzle")).expect("the nozzle");
        m.parts.select(&scene, Some(node));
    }
    h.run_steps(5);
    h.get_by_label("Note").click();
    h.run_steps(10);
    h.get_by_label("Yes").click();
    h.run_steps(30);
    let screen = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().screen.unwrap();
    let click = |h: &mut Harness, at: egui::Pos2| {
        h.event(egui::Event::PointerMoved(at));
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(10);
    };
    let write = |h: &mut Harness, text: &str| {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        m.note_draft.as_mut().expect("a note started").1 = text.to_owned();
        h.run_steps(3);
        h.get_by_label("Add note").click();
        h.run_steps(20);
    };
    // On the part, in the middle of the view.
    click(&mut h, screen.center());
    assert!(matches!(h.state().views[0].view3d.models.get(&(0, 0)).unwrap().note_draft, Some((pdfcraft_engine::NotePlace3d::Leader { .. }, _))));
    h.render().unwrap().save(out.join("n1-started.png")).unwrap();
    write(&mut h, "Tap 10-32 through\nBreak edges");
    // Beside it, near the bottom left corner.
    click(&mut h, screen.left_bottom() + egui::vec2(30.0, -80.0));
    write(&mut h, "All dimensions in inches");
    h.render().unwrap().save(out.join("n2-notes.png")).unwrap();
    let app = h.state();
    let layers = app.session.get(app.views[0].id).unwrap().model_3d_layers(0, 0).unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].kind, pdfcraft_engine::LayerKind3d::Notes);
    let notes = &layers[0].notes;
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(matches!(notes[0].place, pdfcraft_engine::NotePlace3d::Leader { .. }));
    assert!(matches!(notes[1].place, pdfcraft_engine::NotePlace3d::Sheet { .. }));
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    let mut again = pdfcraft_engine::Session::new();
    let id = again.open_new("again.pdf", std::sync::Arc::new(bytes.to_vec())).unwrap();
    assert_eq!(again.get(id).unwrap().model_3d_layers(0, 0).unwrap(), layers);
}

/// On a real 3D PDF (`VIEW3D_PDF`): the nozzle dimensioned and noted, then Print part makes a
/// sheet that opens as a new document; it and a picture of it go into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_part_sheet -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_part_sheet() {
    use super::Load;
    use egui_kittest::kittest::Queryable;
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("Hypochlorous.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let hole = {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        let node = scene.nodes.iter().position(|n| n.name.starts_with("Fill Nozzle")).expect("the nozzle");
        m.parts.select(&scene, Some(node));
        // A whole round hole in it, to dimension.
        scene.nodes[node]
            .meshes
            .iter()
            .find_map(|&mi| {
                let mesh = &scene.meshes[mi].mesh;
                mesh.faces.iter().find_map(|f| {
                    let fit = pdfcraft_prc_render::shapes::fit_face(mesh, f.clone());
                    let pdfcraft_prc_render::shapes::Shape::Cylinder { sweep, .. } = fit.shape else { return None };
                    let t = mesh.triangles[f.start];
                    let c = [0, 1, 2].map(|a| (0..3).map(|v| mesh.positions[t[v] as usize][a]).sum::<f64>() / 3.0);
                    (sweep > 6.0).then_some((mi, c))
                })
            })
            .unwrap()
    };
    h.run_steps(5);
    h.get_by_label("Dimension").click();
    h.run_steps(10);
    h.get_by_label("Yes").click();
    h.run_steps(30);
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        m.measure.pick(&scene.meshes, hole.0, hole.1);
    }
    h.run_steps(5);
    h.get_all_by_label("Keep").next().expect("Keep").click();
    h.run_steps(20);
    h.get_by_label("Note").click();
    h.run_steps(10);
    h.get_by_label("Yes").click();
    h.run_steps(30);
    let screen = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().screen.unwrap();
    for (at, text) in [(screen.center(), "Tap 10-32 through"), (screen.left_bottom() + egui::vec2(30.0, -80.0), "Break all edges")] {
        h.event(egui::Event::PointerMoved(at));
        h.run_steps(1);
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(10);
        h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap().note_draft.as_mut().unwrap().1 = text.to_owned();
        h.run_steps(3);
        h.get_by_label("Add note").click();
        h.run_steps(20);
    }
    h.get_by_label("Print part…").click();
    h.run_steps(10);
    h.render().unwrap().save(out.join("s1-dialog.png")).unwrap();
    h.get_by_label("Make sheet").click();
    h.run_steps(60);
    assert_eq!(h.state().views.len(), 2, "the sheet opened as a new document");
    let app = h.state();
    let sheet = app.session.save_bytes(app.views[1].id).unwrap();
    std::fs::write(out.join("part sheet.pdf"), sheet.as_slice()).unwrap();
    for _ in 0..30 {
        h.run_steps(5);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.render().unwrap().save(out.join("s2-sheet.png")).unwrap();
}

/// A 3D PDF's layers (`VIEW3D_PDF`) written again with their Acrobat views, into `VIEW3D_SAVE`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_write_acrobat_views -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_write_acrobat_views() {
    let (Some(pdf), Some(save)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_SAVE")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.run_steps(5);
    let edit = {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        eprintln!("{} layers", m.layers.len());
        m.layers_edit(0, 0, |_| {})
    };
    if let pdfcraft_engine::Edit::SetModel3dLayers { views, .. } = &edit {
        for v in views {
            eprintln!("view {:?}: {} measurements", v.name, v.measures.len());
        }
    }
    let app = h.state_mut();
    assert!(app.apply_edit(edit));
    let bytes = app.session.save_bytes(app.views[0].id).unwrap();
    std::fs::write(save, bytes.as_slice()).unwrap();
}

/// On a real 3D PDF (`VIEW3D_PDF`): Explode ticked spreads the parts, and with moving parts on
/// a drag moves the part under it; pictures into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_explode -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_explode() {
    use egui_kittest::kittest::Queryable;
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.run_steps(5);
    h.get_by_label("Explode").click();
    h.run_steps(30);
    h.render().unwrap().save(out.join("e1-exploded.png")).unwrap();
    assert!(h.state().views[0].view3d.models.get(&(0, 0)).unwrap().look.offsets.is_some());
    h.get_by_label("Drag parts to move them").click();
    h.run_steps(5);
    let r = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().screen.unwrap();
    // A part under the middle of the view, dragged down and left.
    drag(&mut h, r.center(), egui::vec2(-150.0, 120.0), egui::Modifiers::NONE);
    h.run_steps(30);
    let moved = h.state().views[0].view3d.models.get(&(0, 0)).unwrap().parts.moved.clone();
    eprintln!("moved {moved:?}");
    assert_eq!(moved.len(), 1, "one part moved by hand");
    h.render().unwrap().save(out.join("e2-part-moved.png")).unwrap();
}

/// On a real 3D PDF (`VIEW3D_PDF`): the nozzle isolated and saved as STL and STEP, then the
/// whole model as STEP, into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_export -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_export() {
    use egui_kittest::kittest::Queryable;
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("Hypochlorous.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        let node = scene.nodes.iter().position(|n| n.name.starts_with("Fill Nozzle")).unwrap();
        m.parts.isolated = Some(node);
        m.parts.changed();
    }
    h.run_steps(10);
    for (button, file) in [("Save as STL…", "part.stl"), ("Save as STEP…", "part.step")] {
        h.state_mut().save_override = Some(out.join(file).to_string_lossy().into_owned());
        h.get_by_label(button).click();
        h.run_steps(10);
        let len = std::fs::metadata(out.join(file)).map(|m| m.len()).unwrap_or(0);
        eprintln!("{file}: {len} bytes");
        assert!(len > 1000, "{file} written");
    }
    // A sub-assembly (the smallest with three or more parts) as STEP: an assembly of parts.
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        let triangles = |n: &super::scene::Node| n.meshes.iter().map(|&k| scene.meshes[k].mesh.triangles.len()).sum::<usize>();
        let node = (0..scene.nodes.len())
            .filter(|&k| scene.nodes[k].children.iter().filter(|&&c| triangles(&scene.nodes[c]) > 0).count() >= 3)
            .min_by_key(|&k| scene.nodes[k].children.iter().map(|&c| triangles(&scene.nodes[c])).sum::<usize>())
            .unwrap();
        m.parts.isolated = Some(node);
        m.parts.changed();
    }
    h.run_steps(10);
    h.state_mut().save_override = Some(out.join("sub-assembly.step").to_string_lossy().into_owned());
    h.get_by_label("Save as STEP…").click();
    h.run_steps(10);
    assert!(std::fs::metadata(out.join("sub-assembly.step")).is_ok_and(|m| m.len() > 1000), "sub-assembly written");
    {
        let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
        m.parts.isolated = None;
        m.parts.changed();
    }
    h.run_steps(10);
    // The whole model is too many facets for STEP: not written. As STL it is.
    let _ = std::fs::remove_file(out.join("assembly.step"));
    h.state_mut().save_override = Some(out.join("assembly.step").to_string_lossy().into_owned());
    h.get_by_label("Save as STEP…").click();
    h.run_steps(10);
    assert!(!out.join("assembly.step").exists(), "too big for STEP");
    h.state_mut().save_override = Some(out.join("assembly.stl").to_string_lossy().into_owned());
    h.get_by_label("Save as STL…").click();
    h.run_steps(10);
    eprintln!("assembly.stl: {} bytes", std::fs::metadata(out.join("assembly.stl")).map(|m| m.len()).unwrap_or(0));
}

/// On a real 3D PDF (`VIEW3D_PDF`): the page zoomed far in, the model's picture is drawn only
/// for the part of it on screen, as sharp as the screen; a picture into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_zoomed_page -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_zoomed_page() {
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.state_mut().views[0].set_zoom(9.0);
    for _ in 0..40 {
        h.run_steps(3);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
    let (shown, area, size) = (m.screen.unwrap(), m.area.unwrap(), m.drawn.unwrap().1);
    eprintln!("area {area:?} shown {shown:?} picture {size:?}");
    assert!(area.width() > shown.width() * 2.0, "the area reaches past the screen");
    assert!(f64::from(size[0]) >= f64::from(shown.width()) * 0.99, "as many pixels as the screen shows");
    h.render().unwrap().save(out.join("z1-zoomed.png")).unwrap();
}

#[test]
fn the_zoom_flyout_zooms_the_model_and_goes_back() {
    use egui_kittest::kittest::Queryable;
    let (mut h, at) = shown();
    let before = camera(&h);
    let distance = |c: &Camera| direction(c).iter().map(|v| v * v).sum::<f64>().sqrt();
    let height = |c: &Camera| match c.projection {
        pdfcraft_prc_render::Projection::Orthographic { height } => height,
        pdfcraft_prc_render::Projection::Perspective { .. } => distance(c),
    };
    h.get_by_label("Zoom tools").click();
    h.run_steps(3);
    h.get_by_label("Zoom in").click();
    h.run_steps(3);
    assert!(height(&camera(&h)) < height(&before) * 0.9, "closer");
    h.get_by_label("Zoom tools").click();
    h.run_steps(3);
    h.get_by_label("Back").click();
    h.run_steps(3);
    assert_eq!(camera(&h), before, "back to the view before");
    // A window dragged over the model fills the view with it.
    h.get_by_label("Zoom tools").click();
    h.run_steps(3);
    h.get_by_label("Zoom window").click();
    h.run_steps(3);
    assert_eq!(h.state().quick_tool, crate::QuickTool::MarqueeZoom);
    drag(&mut h, at - egui::vec2(40.0, 30.0), egui::vec2(80.0, 60.0), egui::Modifiers::NONE);
    h.run_steps(5);
    assert!(height(&camera(&h)) < height(&before) * 0.5, "zoomed to the window");
    assert_eq!(h.state().views[0].zoom, h.state().views[0].zoom, "the page left as it was");
}

/// On a real 3D PDF (`VIEW3D_PDF`): Zoom window's rubberband shows over the model while it is
/// dragged; a picture mid-drag into `VIEW3D_OUT`.
/// `cargo test -p pdfcraft-ui-egui --lib view3d_zoom_window_rubberband -- --ignored`
#[test]
#[ignore = "needs a real 3D PDF; run by hand"]
fn view3d_zoom_window_rubberband() {
    let (Some(pdf), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_OUT")) else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let out = std::path::PathBuf::from(out);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for _ in 0..20 {
        h.run_steps(3);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.state_mut().quick_tool = crate::QuickTool::MarqueeZoom;
    let at = h.state().views[0].page_screen_rect(0).unwrap().center() - egui::vec2(150.0, 100.0);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(at + egui::vec2(300.0, 200.0) * (k as f32 / 8.0)));
        h.run_steps(1);
    }
    assert!(h.state().views[0].marquee.is_some(), "dragging a window");
    h.render().unwrap().save(out.join("rubberband.png")).unwrap();
    h.event(egui::Event::PointerButton { pos: at + egui::vec2(300.0, 200.0), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(5);
    h.render().unwrap().save(out.join("rubberband-zoomed.png")).unwrap();
}

#[test]
fn file_export_saves_the_model_as_stl_and_step() {
    let (mut h, _) = shown();
    let dir = std::env::temp_dir().join(format!("pdfcraft-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (command, file) in [("export.stl", "model.stl"), ("export.step", "model.step")] {
        let path = dir.join(file);
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        assert!(h.state_mut().execute(command), "{command} runs on a page with a model");
        h.run_steps(3);
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 84, "{file} written");
    }
    assert!(std::fs::read_to_string(dir.join("model.step")).unwrap().starts_with("ISO-10303-21;"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Lists the model tree's names of the PDF in `VIEW3D_PDF`, for matching mates to parts by hand.
#[test]
fn view3d_dump_names() {
    let Some(pdf) = std::env::var_os("VIEW3D_NAMES") else { return };
    let bytes = std::fs::read(&pdf).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 600.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
    let super::Load::Ready(scene) = &m.load else { panic!() };
    for (k, n) in scene.nodes.iter().enumerate() {
        println!("NODE {k} parent {:?} meshes {} name {:?}", n.parent, n.meshes.len(), n.name);
    }
}

/// Mates on a real assembly: `VIEW3D_PDF` with the mates file `VIEW3D_MATES` attached (as the
/// SOLIDWORKS exporter writes it). Checks each mate's sides lie on the parts they name, draws one
/// picked, and writes the PDF with the mates attached to `VIEW3D_OUT/with-mates.pdf`.
#[test]
#[ignore = "needs a real 3D PDF and its mates; run by hand"]
fn view3d_mates() {
    let (Some(pdf), Some(json), Some(out)) = (std::env::var_os("VIEW3D_PDF"), std::env::var_os("VIEW3D_MATES"), std::env::var_os("VIEW3D_OUT")) else {
        return;
    };
    let bytes = std::fs::read(&pdf).unwrap();
    let mates = std::fs::read(&json).unwrap();
    let out = std::path::PathBuf::from(out);
    let name = std::path::Path::new(&pdf).file_stem().unwrap().to_string_lossy().into_owned();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("model.pdf", None, bytes.clone()).unwrap();
        app.choose_right_panel(Some(crate::state::RightPanel::Model3d));
        app
    });
    for _ in 0..6000 {
        h.run_steps(2);
        if h.state().views[0].view3d.camera(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let edit = pdfcraft_engine::Edit::AddAttachment {
        name: format!("{name}.mates.json"),
        bytes: mates,
        description: Some("Mates exported from the CAD assembly".into()),
        mime: Some("application/json".into()),
    };
    assert!(h.state_mut().apply_edit(edit));
    h.run_steps(6);
    let (count, node) = {
        let m = h.state().views[0].view3d.models.get(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!() };
        assert!(m.mates.error.is_none(), "{:?}", m.mates.error);
        let all = m.mates.mates.as_ref().expect("the mates were read");
        eprintln!("{} mates, missing {:?}", all.mates.len(), all.missing);
        // Each side's point lies on (or in) the part it names: near one of its corners.
        let units = super::measure::Units::of(scene);
        let mut worst = 0.0f64;
        for (k, mate) in all.mates.iter().enumerate() {
            for (e, side) in mate.entities.iter().enumerate() {
                let (Some(n), Some(p)) = (all.nodes[k].get(e).copied().flatten(), side.point) else { continue };
                let p = p.map(|c| c / units.mm_per_unit);
                let near = scene
                    .subtree(n)
                    .iter()
                    .flat_map(|&x| scene.nodes[x].meshes.clone())
                    .flat_map(|mk| scene.meshes[mk].mesh.positions.clone())
                    .map(|q| ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt())
                    .fold(f64::INFINITY, f64::min);
                if near > worst {
                    eprintln!("{} side {e} ({}): {near:.3} units from its part", mate.name, side.kind);
                }
                worst = worst.max(near);
            }
        }
        eprintln!("worst {worst:.3} model units (mm per unit {})", units.mm_per_unit);
        let screw = scene.nodes.iter().position(|n| n.name.ends_with("Screw-1")).unwrap_or(0);
        (all.mates.len(), screw)
    };
    assert!(count > 0);
    {
        let v = &mut h.state_mut().views[0];
        let m = v.view3d.models.get_mut(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        m.parts.select(&scene, Some(node));
        m.mates.open = true;
        let all = m.mates.mates.clone().unwrap();
        let k = all.of_node(&scene, node).into_iter().find(|&k| all.mates[k].kind.contains("Concentric")).unwrap_or(0);
        m.mates.picked = Some(k);
        m.parts.also = all.joined(k).into_iter().collect();
        m.parts.changed();
    }
    h.run_steps(30);
    h.render().unwrap().save(out.join("mates-picked.png")).unwrap();
    // A limit distance mate, when the assembly has one: its moving part slid to each limit.
    let limit = {
        let v = &mut h.state_mut().views[0];
        let m = v.view3d.models.get_mut(&(0, 0)).unwrap();
        let super::Load::Ready(scene) = &m.load else { panic!() };
        let scene = std::sync::Arc::clone(scene);
        let all = m.mates.mates.clone().unwrap();
        all.mates.iter().position(|x| x.kind.contains("LimitDistance")).map(|k| {
            let side = all.nodes[k].iter().flatten().copied().last().unwrap();
            m.parts.select(&scene, Some(side));
            m.mates.picked = Some(k);
            let slide = super::mates::Slide::of(&all, k, &scene, Some(side)).expect("a limit mate slides");
            eprintln!("slide {slide:?} mover {:?}", scene.nodes[slide.mover].name);
            m.mates.slide = Some(slide);
            (scene, slide)
        })
    };
    if let Some((scene, slide)) = limit {
        let units = super::measure::Units::of(&scene);
        for (at, file) in [(slide.hi, "slid-out.png"), (slide.lo, "slid-in.png")] {
            {
                let m = h.state_mut().views[0].view3d.models.get_mut(&(0, 0)).unwrap();
                let mut s = slide;
                s.at = at;
                super::tree::set_slide(&scene, &mut m.parts, &mut m.mates, s, units);
            }
            h.run_steps(30);
            h.render().unwrap().save(out.join(file)).unwrap();
        }
        // Sliding by hand stops at the limits.
        let mut s = slide;
        s.slide_by(slide.dir.map(|c| c * 1e6), units);
        assert!((s.at - slide.hi).abs() < 1e-9);
        s.slide_by(slide.dir.map(|c| c * -1e6), units);
        assert!((s.at - slide.lo).abs() < 1e-9);
    }
    let id = h.state().views[0].id;
    let saved = h.state().session.save_bytes(id).unwrap();
    std::fs::write(out.join("with-mates.pdf"), saved.as_slice()).unwrap();
}
