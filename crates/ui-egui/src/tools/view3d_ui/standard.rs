//! The layers as Acrobat shows them (`pdfcraft_engine::StandardView3d`): a 3D view for each,
//! on one copy of its part, turned to the layer's view with the rest of the assembly hidden, and
//! its dimensions and notes in world coordinates.

use pdfcraft_engine::{DimKind3d, Layer3d, NotePlace3d, StandardMeasure3d, StandardView3d};
use pdfcraft_prc_render::Projection;

use super::dims;
use super::measure::Units;
use super::scene::Scene;

/// The views for `layers`, written with them.
pub(crate) fn standard_views(scene: &Scene, layers: &[Layer3d], units: Units) -> Vec<StandardView3d> {
    layers.iter().filter_map(|l| view_of(scene, l, units)).collect()
}

fn view_of(scene: &Scene, layer: &Layer3d, units: Units) -> Option<StandardView3d> {
    if layer.dims.is_empty() && layer.notes.is_empty() {
        return None;
    }
    // The first copy of the part that draws anything.
    let node = (0..scene.nodes.len()).find(|&k| {
        dims::part_name(scene, k) == layer.part && scene.subtree(k).iter().any(|&n| scene.nodes.get(n).is_some_and(|x| !x.meshes.is_empty()))
    })?;
    let frame = dims::frame_of(scene, node);
    let camera = frame.camera(layer.view.as_ref()?);
    let inch = (units.mm_per_unit - 25.4).abs() < 1e-9;
    let (unit, scale) = if inch { ("in", 1.0) } else { ("mm", units.mm_per_unit) };
    let world = |p: [f64; 3]| frame.world_of(p);
    let mut measures = Vec::new();
    for d in &layer.dims {
        let label = dims::dim_text(d, units);
        let after = tolerance(d.plus * scale, d.minus * scale, usize::from(d.places));
        let text_at = world(d.text_at);
        match (d.kind, d.points.as_slice()) {
            (DimKind3d::Distance, &[a, b]) => measures.push(StandardMeasure3d::Linear {
                from: world(a),
                to: world(b),
                text_at,
                value: d.value * scale,
                units: unit.into(),
                places: d.places,
                after,
                label,
            }),
            (DimKind3d::Diameter | DimKind3d::Radius, &[c, on]) => measures.push(StandardMeasure3d::Radial {
                centre: world(c),
                on: world(on),
                text_at,
                value: d.value * scale,
                radius: d.kind == DimKind3d::Radius,
                units: unit.into(),
                places: d.places,
                after,
                label,
            }),
            // Angles and arc lengths as notes saying what they read.
            (_, &[at, ..]) => measures.push(StandardMeasure3d::Comment { anchor: world(at), text_at, text: label }),
            _ => {}
        }
    }
    for n in &layer.notes {
        let (anchor, text_at) = match n.place {
            NotePlace3d::Leader { anchor, text_at } => (world(anchor), world(text_at)),
            // A note on the sheet has no place on the part: at the middle of the view.
            NotePlace3d::Sheet { .. } => (camera.target, camera.target),
        };
        measures.push(StandardMeasure3d::Comment { anchor, text_at, text: n.text.clone() });
    }
    let kind = if layer.kind == pdfcraft_engine::LayerKind3d::Notes { "Notes" } else { "Dimensions" };
    let (ortho_height, fov_y) = match camera.projection {
        Projection::Orthographic { height } => (Some(height), 30.0),
        Projection::Perspective { fov_y } => (None, fov_y),
    };
    Some(StandardView3d {
        name: format!("{} - {kind}", layer.part),
        eye: camera.eye,
        target: camera.target,
        up: camera.up,
        ortho_height,
        fov_y,
        hide: hidden_around(scene, node),
        measures,
    })
}

/// The tolerance as text after the value: " ±0.005", " +0.005/-0.002", or none.
fn tolerance(plus: f64, minus: f64, places: usize) -> String {
    if plus == 0.0 && minus == 0.0 {
        String::new()
    } else if (plus - minus).abs() < 1e-12 {
        format!(" ±{plus:.places$}")
    } else {
        format!(" +{plus:.places$}/-{minus:.places$}")
    }
}

/// The nodes to hide so only `node` shows: beside it and beside each node above it, by name.
fn hidden_around(scene: &Scene, node: usize) -> Vec<String> {
    let mut hide = Vec::new();
    let mut here = node;
    while let Some(parent) = scene.nodes.get(here).and_then(|n| n.parent) {
        for &k in scene.nodes.get(parent).map_or(&[][..], |p| p.children.as_slice()) {
            if k != here
                && let Some(n) = scene.nodes.get(k)
                && !n.name.starts_with("occurrence ")
            {
                hide.push(n.name.clone());
            }
        }
        here = parent;
    }
    hide
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerances_follow_the_value() {
        assert_eq!(tolerance(0.0, 0.0, 3), "");
        assert_eq!(tolerance(0.005, 0.005, 3), " ±0.005");
        assert_eq!(tolerance(0.005, 0.002, 3), " +0.005/-0.002");
    }
}
