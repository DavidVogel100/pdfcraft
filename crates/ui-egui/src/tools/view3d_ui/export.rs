//! Export: what the model shows (not the parts hidden; the part isolated in its own
//! coordinates) as an STL or STEP file, in millimetres, the parts where the model places them
//! (an exploded view is not exported apart).

use pdfcraft_prc_render::Look;
use pdfcraft_prc_render::export::{ExportPart, step, stl};

use super::dims;
use super::measure::Units;
use super::scene::Scene;

/// The most triangles a STEP file is made of: on curved surfaces each is a face of its own with
/// its edges and loop, about 1 KB, so this is some 100 MB.
const MAX_STEP_TRIANGLES: usize = 100_000;

/// The file for what `look` shows of `scene`: its name and bytes, STEP when `as_step`, else
/// STL. Named after the part isolated, else `file` (the PDF).
///
/// # Errors
/// A STEP file of more than [`MAX_STEP_TRIANGLES`]: what to do instead.
pub(crate) fn export(scene: &Scene, look: &Look, isolated: Option<usize>, file: &str, as_step: bool) -> Result<(String, Vec<u8>), String> {
    let parts = parts(scene, look, isolated);
    let count: usize = parts.iter().map(|p| p.triangles.len()).sum();
    if as_step && count > MAX_STEP_TRIANGLES {
        return Err(crate::i18n::fmt(
            tl!("What is shown has {n} facets, too many for a STEP file of a useful size. Isolate a part or a sub-assembly, or save it as STL."),
            &[("n", &count.to_string())],
        ));
    }
    let stem = match isolated {
        Some(node) => dims::part_name(scene, node),
        None => file.trim_end_matches(".pdf").trim_end_matches(".PDF").to_owned(),
    };
    let stem = if stem.trim().is_empty() { "model".to_owned() } else { stem };
    // Characters a file name can't hold.
    let safe: String = stem.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    if as_step {
        Ok((format!("{safe}.step"), step(&stem, &super::sheet::today(), &parts).into_bytes()))
    } else {
        Ok((format!("{safe}.stl"), stl(&stem, &parts)))
    }
}

/// The shown meshes' triangles, one part per tree node, in millimetres: in the isolated node's
/// own coordinates, else the world's.
fn parts(scene: &Scene, look: &Look, isolated: Option<usize>) -> Vec<ExportPart> {
    let frame = isolated.map(|n| dims::frame_of(scene, n));
    let mm = Units::of(scene).mm_per_unit;
    let mut by_node: Vec<(Option<usize>, ExportPart)> = Vec::new();
    for (k, m) in scene.meshes.iter().enumerate() {
        if look.hidden.get(k).copied().unwrap_or(false) {
            continue;
        }
        let node = scene.mesh_nodes.get(k).copied().flatten();
        let at = by_node.iter().position(|(n, _)| *n == node).unwrap_or_else(|| {
            let name = node.and_then(|n| scene.nodes.get(n)).map_or_else(|| format!("Part {}", k + 1), |n| n.name.clone());
            by_node.push((node, ExportPart { name, triangles: Vec::new(), colours: Vec::new() }));
            by_node.len() - 1
        });
        let place = |p: [f64; 3]| frame.map_or(p, |f| f.local_of(p)).map(|c| c * mm);
        let corner = |i: u32| usize::try_from(i).ok().and_then(|i| m.mesh.positions.get(i).copied()).filter(|p| p.iter().all(|c| c.is_finite()));
        if let Some((_, part)) = by_node.get_mut(at) {
            for (k, t) in m.mesh.triangles.iter().enumerate() {
                let (Some(a), Some(b), Some(c)) = (corner(t[0]), corner(t[1]), corner(t[2])) else { continue };
                part.triangles.push([place(a), place(b), place(c)]);
                // The colour it shows in, as the viewer's default when it has none.
                let [r, g, b, _] = m.colour_of(k).unwrap_or([190, 192, 200, 255]);
                part.colours.push([r, g, b]);
            }
        }
    }
    by_node.into_iter().map(|(_, p)| p).filter(|p| !p.triangles.is_empty()).collect()
}
