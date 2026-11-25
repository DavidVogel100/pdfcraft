//! Mates: how the parts of an assembly are joined, read from a `*.mates.json` attachment that a
//! CAD program's exporter (`tools/cad-mates`) wrote, since a 3D PDF itself holds no mates. Each
//! mate names its components as the CAD program does (`Sub-1/Part-2`), which are found in the
//! model tree by the same names; its axes and planes are in millimetres, in the assembly's own
//! coordinates, which a 3D PDF keeps.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Stroke, pos2};
use pdfcraft_prc_render::Camera;
use serde::Deserialize;

use super::measure::Units;
use super::scene::Scene;

/// The most mates read from a file.
const MAX_MATES: usize = 20_000;

#[derive(Deserialize, Default, Clone, Debug)]
struct File {
    #[serde(default)]
    format: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    mates: Vec<Mate>,
}

/// One mate as the exporter wrote it.
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Mate {
    #[serde(default)]
    pub(crate) name: String,
    /// The CAD program's own type name: `MateConcentric`, `MateDistanceDim`, …
    #[serde(rename = "type", default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) alignment: Option<String>,
    #[serde(default)]
    pub(crate) suppressed: bool,
    /// Distance (mm) or angle (degrees), and a limit mate's range.
    #[serde(default)]
    pub(crate) value: Option<f64>,
    #[serde(default)]
    pub(crate) min: Option<f64>,
    #[serde(default)]
    pub(crate) max: Option<f64>,
    #[serde(default)]
    pub(crate) entities: Vec<Entity>,
}

/// What a mate holds on one side: a face, edge, axis or plane of a component.
#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Entity {
    /// The component, as the CAD program names it; `None` for the assembly's own planes.
    #[serde(default)]
    pub(crate) component: Option<String>,
    #[serde(default)]
    pub(crate) kind: String,
    /// A point on it and its axis or normal, in mm.
    #[serde(default)]
    pub(crate) point: Option<[f64; 3]>,
    #[serde(default)]
    pub(crate) direction: Option<[f64; 3]>,
    #[serde(default)]
    pub(crate) radius: Option<f64>,
}

/// A model's mates, with the tree node each side belongs to.
#[derive(Clone, Debug, Default)]
pub(crate) struct Mates {
    /// The attachment they came from, and the program that wrote it.
    pub(crate) file: String,
    pub(crate) source: String,
    pub(crate) mates: Vec<Mate>,
    /// Per mate, per entity: its node, when the component was found.
    pub(crate) nodes: Vec<Vec<Option<usize>>>,
    /// Components named by mates that the model tree has no part for.
    pub(crate) missing: Vec<String>,
}

impl Mates {
    /// Reads the exporter's JSON `text` (from attachment `file`) and finds its components in
    /// `scene`. Why not, when it is not a mates file.
    pub(crate) fn read(file: &str, text: &str, scene: &Scene) -> Result<Mates, String> {
        let f: File = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if f.format != "pdfcraft-mates" {
            return Err(tl!("the attachment is not a mates file").to_owned());
        }
        let paths = node_paths(scene);
        let mut missing = Vec::new();
        let mut mates = f.mates;
        mates.truncate(MAX_MATES);
        let nodes = mates
            .iter()
            .map(|m| {
                m.entities
                    .iter()
                    .map(|e| {
                        let name = e.component.as_deref()?;
                        let found = find(&paths, name);
                        // The assembly's own planes name the assembly, not a component.
                        let component = name.rsplit('/').next().is_some_and(is_component);
                        if found.is_none() && component && !missing.iter().any(|x| x == name) {
                            missing.push(name.to_owned());
                        }
                        found
                    })
                    .collect()
            })
            .collect();
        Ok(Mates { file: file.to_owned(), source: f.source, mates, nodes, missing })
    }

    /// The mates that hold `node` or a part under it, in file order.
    pub(crate) fn of_node(&self, scene: &Scene, node: usize) -> Vec<usize> {
        let under = scene.subtree(node);
        (0..self.mates.len()).filter(|&k| self.nodes.get(k).is_some_and(|ns| ns.iter().flatten().any(|n| under.contains(n)))).collect()
    }

    /// The nodes mate `k` joins.
    pub(crate) fn joined(&self, k: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self.nodes.get(k).map(|ns| ns.iter().flatten().copied().collect()).unwrap_or_default();
        v.dedup();
        v
    }
}

/// Per node, its path as a CAD program names a component: the names of the component nodes from
/// the top of the assembly down to it (`Sub-1/Part-2`). Nodes that are not components (the
/// assembly itself, a configuration) are left out of the path.
fn node_paths(scene: &Scene) -> Vec<String> {
    let mut paths: Vec<String> = Vec::with_capacity(scene.nodes.len());
    for (k, n) in scene.nodes.iter().enumerate() {
        let parent = n.parent.filter(|&p| p < k).and_then(|p| paths.get(p)).cloned().unwrap_or_default();
        let own = is_component(&n.name).then_some(n.name.as_str());
        paths.push(match (parent.is_empty(), own) {
            (_, None) => parent,
            (true, Some(o)) => o.to_owned(),
            (false, Some(o)) => format!("{parent}/{o}"),
        });
    }
    paths
}

/// Whether a node's name is a component's: a name with an instance number, `Part-2`.
fn is_component(name: &str) -> bool {
    name.rsplit_once('-').is_some_and(|(base, n)| !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The node whose path is `name`; else the one node whose own name is the last part of it.
fn find(paths: &[String], name: &str) -> Option<usize> {
    let want = name.replace('\\', "/");
    if let Some(k) = paths.iter().position(|p| p.eq_ignore_ascii_case(&want)) {
        return Some(k);
    }
    let last = want.rsplit('/').next().unwrap_or(&want);
    let ends: Vec<usize> = paths.iter().enumerate().filter(|(_, p)| p.rsplit('/').next().is_some_and(|l| l.eq_ignore_ascii_case(last))).map(|(k, _)| k).collect();
    match ends.as_slice() {
        [one] => Some(*one),
        // Several copies of a sub-assembly: the one whose path ends with the whole name.
        _ => ends.into_iter().find(|&k| paths.get(k).is_some_and(|p| p.to_ascii_lowercase().ends_with(&want.to_ascii_lowercase()))),
    }
}

/// A mate's type as SOLIDWORKS' Mates folder names it: "Concentric", "Distance", "Limit distance".
pub(crate) fn type_name(m: &Mate) -> &'static str {
    let t = m.kind.trim_start_matches("Mate");
    match t {
        "Coincident" => tl!("Coincident"),
        "Concentric" => tl!("Concentric"),
        "Parallel" => tl!("Parallel"),
        "Perpendicular" => tl!("Perpendicular"),
        "Tangent" => tl!("Tangent"),
        "DistanceDim" | "Distance" => tl!("Distance"),
        "AngleDim" | "Angle" => tl!("Angle"),
        "LimitDistanceDim" | "LimitDistance" => tl!("Limit distance"),
        "LimitAngleDim" | "LimitAngle" => tl!("Limit angle"),
        "Width" => tl!("Width"),
        "Symmetric" => tl!("Symmetric"),
        "Lock" => tl!("Lock"),
        "Slot" => tl!("Slot"),
        "Hinge" => tl!("Hinge"),
        "Gear" => tl!("Gear"),
        "Screw" => tl!("Screw"),
        "CamFollower" | "Cam" => tl!("Cam"),
        "RackPinion" => tl!("Rack and pinion"),
        "Path" => tl!("Path"),
        "LinearCoupler" => tl!("Linear coupler"),
        "UniversalJoint" => tl!("Universal joint"),
        "Coordinate" => tl!("Coordinate"),
        "ProfileCenter" => tl!("Profile center"),
        _ => tl!("Mate"),
    }
}

/// The icon a mate's row shows.
pub(crate) fn icon(m: &Mate) -> &'static str {
    let t = m.kind.trim_start_matches("Mate");
    if t.starts_with("Concentric") || t == "Hinge" || t == "Screw" || t == "Gear" {
        "circle-dot"
    } else if t.contains("Distance") || t == "Width" || t == "Slot" {
        "ruler"
    } else if t.contains("Angle") {
        "rotate-cw"
    } else {
        "link"
    }
}

/// A mate's value as the Mates folder shows it after its name: a distance in the model's units,
/// an angle in degrees, a limit's range; empty when it has none.
pub(crate) fn value_text(m: &Mate, units: Units) -> String {
    let angle = m.kind.contains("Angle");
    let say = |v: f64| if angle { format!("{v:.2}°") } else { units.length(v / units.mm_per_unit.max(1e-12)) };
    match (m.min, m.max, m.value) {
        (Some(lo), Some(hi), _) => format!("{} … {}", say(lo), say(hi)),
        (_, _, Some(v)) => say(v),
        _ => String::new(),
    }
}

/// Where the mate's sides are, in the model's world (its units): per entity, a point and its
/// axis or normal, and its radius.
pub(crate) fn geometry(m: &Mate, units: Units) -> Vec<([f64; 3], [f64; 3], f64, &str)> {
    let per = units.mm_per_unit.max(1e-12);
    m.entities
        .iter()
        .filter_map(|e| {
            let p = e.point?;
            let d = e.direction.unwrap_or([0.0, 0.0, 0.0]);
            Some((p.map(|c| c / per), d, e.radius.unwrap_or(0.0) / per, e.kind.as_str()))
        })
        .collect()
}

/// The mate picked in the Mates list: its axes drawn as lines through the model, its planes as
/// squares, over the model shown in `r` by `camera` into a `size` picture. `reach` is how far the
/// drawing runs (world units).
pub(crate) fn paint(painter: &egui::Painter, r: Rect, size: [u32; 2], camera: &Camera, m: &Mate, units: Units, reach: f64) {
    let to_screen = |p: [f64; 3]| -> Option<Pos2> {
        let [x, y] = camera.project(p, size[0], size[1])?;
        let (w, h) = (f64::from(size[0].max(1)), f64::from(size[1].max(1)));
        Some(pos2(r.left() + (x / w) as f32 * r.width(), r.top() + (y / h) as f32 * r.height()))
    };
    const INK: Color32 = Color32::from_rgb(0xE6, 0x5C, 0x00);
    let stroke = Stroke::new(2.0, INK);
    let thin = Stroke::new(1.0, INK);
    for (p, d, radius, kind) in geometry(m, units) {
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len < 1e-9 {
            if let Some(c) = to_screen(p) {
                painter.circle_stroke(c, 5.0, stroke);
            }
            continue;
        }
        let d = d.map(|c| c / len);
        let along = |t: f64| [p[0] + d[0] * t, p[1] + d[1] * t, p[2] + d[2] * t];
        if kind == "plane" {
            // A square on the plane, the normal standing out of its middle.
            let side = reach * 0.15;
            let any = if d[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let u = unit(cross(d, any));
            let v = cross(d, u);
            let corner = |a: f64, b: f64| [p[0] + (u[0] * a + v[0] * b) * side, p[1] + (u[1] * a + v[1] * b) * side, p[2] + (u[2] * a + v[2] * b) * side];
            let pts: Vec<Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().filter_map(|&(a, b)| to_screen(corner(a, b))).collect();
            if pts.len() == 4 {
                painter.add(egui::Shape::convex_polygon(pts, INK.gamma_multiply(0.18), stroke));
            }
            if let (Some(a), Some(b)) = (to_screen(p), to_screen(along(side))) {
                painter.arrow(a, b - a, thin);
            }
        } else {
            // An axis: a long dashed line, and a ring for a round.
            if let (Some(a), Some(b)) = (to_screen(along(-reach * 0.6)), to_screen(along(reach * 0.6))) {
                painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 8.0, 5.0));
            }
            if radius > 0.0 {
                let any = if d[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
                let u = unit(cross(d, any));
                let v = cross(d, u);
                let ring: Vec<Pos2> = (0..=36)
                    .filter_map(|i| {
                        let a = f64::from(i) / 36.0 * std::f64::consts::TAU;
                        let (s, c) = a.sin_cos();
                        to_screen([0, 1, 2].map(|k| p[k] + (u[k] * c + v[k] * s) * radius))
                    })
                    .collect();
                painter.add(egui::Shape::line(ring, stroke));
            }
        }
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    a.map(|c| c / l)
}

/// The mates of a model as kept with it: read again after every edit (attaching the file is one).
#[derive(Default)]
pub(crate) struct MateState {
    pub(crate) read_at: Option<u64>,
    pub(crate) mates: Option<Arc<Mates>>,
    pub(crate) error: Option<String>,
    /// The mate picked in the list: drawn on the model, its parts highlighted.
    pub(crate) picked: Option<usize>,
    /// The Mates folder (all of them) is open.
    pub(crate) open: bool,
}

impl MateState {
    /// Reads the document's mates again when it changed since.
    pub(crate) fn refresh(&mut self, doc: &pdfcraft_engine::Document, scene: &Scene) {
        let generation = doc.edit_generation();
        if self.read_at == Some(generation) {
            return;
        }
        self.read_at = Some(generation);
        self.error = None;
        self.mates = match doc.model_3d_mates() {
            Some((file, text)) => match Mates::read(&file, &text, scene) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    self.error = Some(e);
                    None
                }
            },
            None => None,
        };
        let count = self.mates.as_ref().map_or(0, |m| m.mates.len());
        self.picked = self.picked.filter(|&k| k < count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::view3d_ui::scene::Node;

    fn scene(names: &[(&str, Option<usize>)]) -> Scene {
        let mut s = Scene::default();
        s.nodes = names.iter().map(|&(n, p)| Node { name: n.to_owned(), parent: p, ..Node::default() }).collect();
        for k in 0..s.nodes.len() {
            if let Some(p) = s.nodes[k].parent {
                s.nodes[p].children.push(k);
            }
        }
        s
    }

    #[test]
    fn components_are_found_by_their_cad_path() {
        let s = scene(&[("Top", None), ("Default", Some(0)), ("Plate-1", Some(1)), ("Sub-1", Some(1)), ("Pin-1", Some(3)), ("Sub-2", Some(1)), ("Pin-1", Some(5))]);
        let text = r#"{"format":"pdfcraft-mates","version":1,"mates":[
            {"name":"Concentric1","type":"MateConcentric","entities":[{"component":"Plate-1","kind":"cylinder"},{"component":"Sub-2/Pin-1","kind":"line"}]},
            {"name":"Coincident1","type":"MateCoincident","entities":[{"component":"Nowhere-1","kind":"plane"},{"component":null,"kind":"plane"}]}]}"#;
        let m = Mates::read("a.mates.json", text, &s).unwrap();
        assert_eq!(m.nodes[0], [Some(2), Some(6)]);
        assert_eq!(m.nodes[1], [None, None]);
        assert_eq!(m.missing, ["Nowhere-1"]);
        assert_eq!(m.of_node(&s, 5), [0]);
        assert_eq!(m.of_node(&s, 3), Vec::<usize>::new());
        assert_eq!(type_name(&m.mates[0]), "Concentric");
    }

    #[test]
    fn other_json_is_refused() {
        assert!(Mates::read("x.mates.json", r#"{"mates":[]}"#, &Scene::default()).is_err());
        assert!(Mates::read("x.mates.json", "not json", &Scene::default()).is_err());
    }
}
