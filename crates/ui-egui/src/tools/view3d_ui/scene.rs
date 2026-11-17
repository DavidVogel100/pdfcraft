//! A 3D model as the viewer holds it: its meshes placed in the world, its assembly tree, and
//! which tree node each mesh belongs to; and the camera moves a drag or the wheel makes.

use craft_prc::PrcFile;
use pdfcraft_prc_render::{Bounds, Camera, Projection, WorldMesh, assemble};

/// One node of the assembly tree, as the Model Tree lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Node {
    /// The name to show; `occurrence F:I` when the file names it nowhere.
    pub(crate) name: String,
    pub(crate) parent: Option<usize>,
    pub(crate) children: Vec<usize>,
    /// The meshes this node draws itself (not those of the nodes under it).
    pub(crate) meshes: Vec<usize>,
}

/// A model read for the viewer.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scene {
    pub(crate) meshes: Vec<WorldMesh>,
    pub(crate) nodes: Vec<Node>,
    /// Per mesh, the node it belongs to.
    pub(crate) mesh_nodes: Vec<Option<usize>>,
    pub(crate) bounds: Option<Bounds>,
    /// Per mesh, the edges drawn over it: where its faces meet.
    pub(crate) outlines: std::sync::Arc<[Vec<[u32; 2]>]>,
    /// The model's unit, when the file says what it is.
    pub(crate) units: Option<craft_prc::ModelUnits>,
}

impl Scene {
    /// Reads a PRC stream: its placed meshes, its tree, and which node draws which mesh.
    pub(crate) fn read(data: &[u8]) -> Result<Scene, String> {
        let prc = PrcFile::parse(data).map_err(|e| e.to_string())?;
        let meshes = assemble(&prc).map_err(|e| e.to_string())?;
        // A file whose tree can't be read is still drawn, with no parts to list.
        let tree = if prc.stores_tree() { prc.model_tree().unwrap_or_default() } else { Vec::new() };
        let mut nodes: Vec<Node> = tree.iter().map(|n| Node { name: n.label(), parent: n.parent, ..Node::default() }).collect();
        for k in 0..nodes.len() {
            if let Some(p) = nodes.get(k).and_then(|n| n.parent).filter(|&p| p < k)
                && let Some(parent) = nodes.get_mut(p)
            {
                parent.children.push(k);
            }
        }
        // A mesh belongs to the deepest node whose placements hold its own: the tree lists a
        // parent before its children, and each node's range holds its children's.
        let mesh_nodes: Vec<Option<usize>> = meshes
            .iter()
            .map(|m| {
                let p = m.placement?;
                tree.iter().enumerate().filter(|(_, n)| n.placements.contains(&p)).max_by_key(|(_, n)| n.depth).map(|(k, _)| k)
            })
            .collect();
        for (mesh, node) in mesh_nodes.iter().enumerate() {
            if let Some(n) = node.and_then(|k| nodes.get_mut(k)) {
                n.meshes.push(mesh);
            }
        }
        let bounds = Bounds::of(&meshes);
        let units = prc.units().ok().flatten();
        let outlines = meshes.iter().map(|m| pdfcraft_prc_render::outline_edges(&m.mesh)).collect();
        Ok(Scene { meshes, nodes, mesh_nodes, bounds, outlines, units })
    }

    /// The nodes with no parent.
    pub(crate) fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter(|(_, n)| n.parent.is_none()).map(|(k, _)| k)
    }

    /// `node` and every node under it.
    pub(crate) fn subtree(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(k) = stack.pop() {
            // The tree comes from the file: a node met twice would loop.
            if out.contains(&k) || out.len() > self.nodes.len() {
                continue;
            }
            if let Some(n) = self.nodes.get(k) {
                out.push(k);
                stack.extend(n.children.iter().rev());
            }
        }
        out
    }
}

/// The world axis nearest `v`, as a unit vector: what a turntable turns about.
pub(crate) fn nearest_axis(v: [f64; 3]) -> [f64; 3] {
    let k = (0..3).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap_or(2);
    let mut axis = [0.0; 3];
    axis[k] = if v[k] < 0.0 { -1.0 } else { 1.0 };
    axis
}

/// Turn the camera as a turntable does, about the world point `about` (the model stays put
/// there on the picture): `dx` round the `pivot` axis and `dy` over the top, both in pixels of a
/// picture `height` high (its height is half a turn). Stops short of looking straight along the
/// pivot, where the view would flip.
pub(crate) fn turn(camera: &mut Camera, pivot: [f64; 3], about: [f64; 3], dx: f64, dy: f64, height: f64) {
    let k = std::f64::consts::PI / height.max(1.0);
    let (yaw, pitch) = (-dx * k, -dy * k);
    // Eye and target, each relative to the point turned about, turned together.
    let (eye, target) = (sub(camera.eye, about), sub(camera.target, about));
    let (eye, target, up) = (rotate(eye, pivot, yaw), rotate(target, pivot, yaw), rotate(camera.up, pivot, yaw));
    let back = sub(eye, target);
    let Some(right) = normalize(cross(up, back)) else { return };
    let (eye, target) =
        if dot(rotate(up, right, pitch), pivot) > 0.02 { (rotate(eye, right, pitch), rotate(target, right, pitch)) } else { (eye, target) };
    camera.eye = add(about, eye);
    camera.target = add(about, target);
    // Kept square to the view, so the renderer's basis always forms.
    let back = sub(eye, target);
    if let Some(up) = normalize(cross(normalize(back).unwrap_or(back), right)) {
        camera.up = up;
    }
}

/// Widen the view so a picture `factor` times as tall (and as wide) shows the model at the size
/// the camera showed it before: the field of view or the orthographic height grows by `factor`.
pub(crate) fn widen(camera: &mut Camera, factor: f64) {
    if !(factor.is_finite() && factor > 0.0) {
        return;
    }
    match &mut camera.projection {
        Projection::Perspective { fov_y } => {
            let wider = 2.0 * ((fov_y.to_radians() / 2.0).tan() * factor).atan().to_degrees();
            if wider > 0.0 && wider < 179.0 {
                *fov_y = wider;
            }
        }
        Projection::Orthographic { height } => *height *= factor,
    }
}

/// Slide the camera and its target so the model follows the pointer: `dx`, `dy` pixels of a
/// picture `height` high.
pub(crate) fn pan(camera: &mut Camera, dx: f64, dy: f64, height: f64) {
    let Some(ahead) = normalize(sub(camera.target, camera.eye)) else { return };
    let Some(right) = normalize(cross(ahead, camera.up)) else { return };
    let up = cross(right, ahead);
    let per_pixel = view_height(camera) / height.max(1.0);
    let shift = add(scale(right, -dx * per_pixel), scale(up, dy * per_pixel));
    camera.eye = add(camera.eye, shift);
    camera.target = add(camera.target, shift);
}

/// Zoom by `factor` (below 1: closer) towards the world point `about`, which stays where it is
/// on the picture: everything moves towards it, or away, along the view.
pub(crate) fn zoom_at(camera: &mut Camera, about: [f64; 3], factor: f64) {
    if !(factor.is_finite() && factor > 0.0) || !about.iter().all(|c| c.is_finite()) {
        return;
    }
    let target = add(about, scale(sub(camera.target, about), factor));
    match &mut camera.projection {
        Projection::Perspective { .. } => {
            let eye = add(about, scale(sub(camera.eye, about), factor));
            if dot(sub(target, eye), sub(target, eye)) > 1e-24 {
                (camera.eye, camera.target) = (eye, target);
            }
        }
        Projection::Orthographic { height } => {
            // The view keeps its distance; it slides across and shows less.
            camera.eye = add(camera.eye, sub(target, camera.target));
            camera.target = target;
            *height = (*height * factor).max(1e-9);
        }
    }
}

/// How far ahead of the eye the target lies, along the view.
pub(crate) fn depth_of_target(camera: &Camera) -> f64 {
    let d = sub(camera.target, camera.eye);
    dot(d, d).sqrt()
}

/// Model units from the bottom of the view to the top, at the target.
fn view_height(camera: &Camera) -> f64 {
    match camera.projection {
        Projection::Perspective { fov_y } => {
            let d = sub(camera.target, camera.eye);
            2.0 * dot(d, d).sqrt() * (fov_y.to_radians() / 2.0).tan()
        }
        Projection::Orthographic { height } => height,
    }
}

/// `v` turned by `angle` radians about the unit `axis` (Rodrigues).
fn rotate(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    let k = dot(axis, v) * (1.0 - c);
    add(add(scale(v, c), scale(cross(axis, v), s)), scale(axis, k))
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    a.map(|v| v * k)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot(a, a).sqrt();
    (len.is_finite() && len > 0.0).then(|| scale(a, 1.0 / len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera { eye: [0.0, -10.0, 0.0], target: [0.0; 3], up: [0.0, 0.0, 1.0], projection: Projection::Perspective { fov_y: 30.0 } }
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    #[test]
    fn a_drag_across_turns_round_the_pivot_and_keeps_the_distance() {
        let mut c = camera();
        // Half the picture's height across is a quarter turn.
        let target = c.target;
        turn(&mut c, [0.0, 0.0, 1.0], target, 100.0, 0.0, 200.0);
        assert!(close(c.eye, [10.0, 0.0, 0.0]) || close(c.eye, [-10.0, 0.0, 0.0]), "{:?}", c.eye);
        assert!(close(c.up, [0.0, 0.0, 1.0]));
    }

    #[test]
    fn a_drag_up_and_down_stops_short_of_the_pole() {
        let mut c = camera();
        for _ in 0..20 {
            let target = c.target;
            turn(&mut c, [0.0, 0.0, 1.0], target, 0.0, 40.0, 200.0);
        }
        assert!(dot(c.up, [0.0, 0.0, 1.0]) > 0.0, "never flipped over: {:?}", c.up);
        let d = sub(c.eye, c.target);
        assert!((dot(d, d).sqrt() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn panning_moves_eye_and_target_together_and_zoom_keeps_the_target() {
        let mut c = camera();
        pan(&mut c, 100.0, 0.0, 100.0);
        assert!(c.target[0] < 0.0 && (c.eye[0] - c.target[0]).abs() < 1e-9, "the model follows the pointer right");
        let target = c.target;
        zoom_at(&mut c, target, 0.5);
        assert!(close(c.target, target));
        let d = sub(c.eye, c.target);
        assert!((dot(d, d).sqrt() - 5.0).abs() < 1e-9);
        zoom_at(&mut c, target, f64::NAN);
        assert!((dot(sub(c.eye, c.target), sub(c.eye, c.target)).sqrt() - 5.0).abs() < 1e-9);
        let mut o = Camera { projection: Projection::Orthographic { height: 4.0 }, ..camera() };
        let target = o.target;
        zoom_at(&mut o, target, 2.0);
        assert_eq!(o.projection, Projection::Orthographic { height: 8.0 });
    }

    #[test]
    fn turning_about_a_point_keeps_that_point_where_it_is_on_the_picture() {
        let mut c = camera();
        // A point off to the side of the target, which a drag started over.
        let about = [2.0, 0.0, 1.0];
        let before = sub(about, c.eye);
        let (ahead, up) = (normalize(sub(c.target, c.eye)).unwrap(), c.up);
        let along = |c: &Camera, v: [f64; 3]| {
            let ahead = normalize(sub(c.target, c.eye)).unwrap();
            let right = normalize(cross(ahead, c.up)).unwrap();
            [dot(v, right), dot(v, c.up), dot(v, ahead)]
        };
        let seen = along(&Camera { up, ..c }, before);
        turn(&mut c, [0.0, 0.0, 1.0], about, 30.0, 20.0, 200.0);
        let after = along(&c, sub(about, c.eye));
        assert!((0..3).all(|k| (seen[k] - after[k]).abs() < 1e-9), "{seen:?} {after:?}");
        assert!(!close(normalize(sub(c.target, c.eye)).unwrap(), ahead), "the view turned");
    }

    #[test]
    fn widening_keeps_the_model_the_same_size_on_a_taller_picture() {
        let mut c = camera();
        widen(&mut c, 2.0);
        let doubled = |fov_y: f64| ((fov_y.to_radians() / 2.0).tan() - 2.0 * (15f64.to_radians()).tan()).abs() < 1e-12;
        assert!(matches!(c.projection, Projection::Perspective { fov_y } if doubled(fov_y)), "{:?}", c.projection);
        let mut o = Camera { projection: Projection::Orthographic { height: 4.0 }, ..camera() };
        widen(&mut o, 1.5);
        assert_eq!(o.projection, Projection::Orthographic { height: 6.0 });
    }

    #[test]
    fn zooming_towards_a_point_keeps_it_where_it_is_on_the_picture() {
        for projection in [Projection::Perspective { fov_y: 30.0 }, Projection::Orthographic { height: 4.0 }] {
            let mut c = Camera { projection, ..camera() };
            let about = [1.0, 2.0, 0.7];
            let before = c.project(about, 400, 300).unwrap();
            zoom_at(&mut c, about, 0.5);
            let after = c.project(about, 400, 300).unwrap();
            assert!((before[0] - after[0]).abs() < 1e-6 && (before[1] - after[1]).abs() < 1e-6, "{before:?} {after:?}");
        }
    }

    #[test]
    fn the_pivot_is_the_world_axis_nearest_the_view_up() {
        assert_eq!(nearest_axis([0.1, 0.2, 0.9]), [0.0, 0.0, 1.0]);
        assert_eq!(nearest_axis([0.1, -0.9, 0.2]), [0.0, -1.0, 0.0]);
    }

    #[test]
    fn a_subtree_lists_a_node_and_everything_under_it_once() {
        let node = |parent: Option<usize>, children: Vec<usize>| Node { name: String::new(), parent, children, meshes: Vec::new() };
        let s =
            Scene { nodes: vec![node(None, vec![1, 2]), node(Some(0), vec![3]), node(Some(0), vec![]), node(Some(1), vec![0])], ..Scene::default() };
        assert_eq!(s.subtree(1), [1, 3, 0, 2]);
        assert_eq!(s.roots().collect::<Vec<_>>(), [0]);
    }
}
