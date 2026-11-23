//! Parts written out as files other programs read: binary STL (for 3D printing) and STEP
//! (ISO 10303-21, AP214). A 3D PDF carries the parts' triangles, not their CAD solids, so the
//! STEP faces are flat: triangles that lie in one plane (and share a colour) are merged into one
//! face, holes and all. A part whose skin is closed becomes a solid (`MANIFOLD_SOLID_BREP`), which
//! CAD programs import as a body to measure and mate to; any other a surface body. Parts keep their
//! colours, and several parts become an assembly of them. Coordinates are written as given, in
//! millimetres.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::vec3::{cross, dot, length, normalize, scale, sub};

/// One part to write: its name, its triangles in millimetres and, per triangle, its colour
/// (missing ones are grey).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportPart {
    pub name: String,
    pub triangles: Vec<[[f64; 3]; 3]>,
    pub colours: Vec<[u8; 3]>,
}

/// The colour of a triangle with none.
const GREY: [u8; 3] = [190, 192, 200];

/// The parts as one binary STL: an 80-byte header naming `title`, then every triangle.
#[must_use]
pub fn stl(title: &str, parts: &[ExportPart]) -> Vec<u8> {
    let count: usize = parts.iter().map(|p| p.triangles.len()).sum();
    let mut out = Vec::with_capacity(84 + count * 50);
    let mut header = [b' '; 80];
    for (slot, b) in header.iter_mut().zip(format!("pdfcraft: {title}").bytes().filter(u8::is_ascii)) {
        *slot = b;
    }
    // Binary STL must not start with "solid", which marks the text form.
    if header.starts_with(b"solid") {
        header[0] = b'S';
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&u32::try_from(count).unwrap_or(u32::MAX).to_le_bytes());
    for t in parts.iter().flat_map(|p| p.triangles.iter()) {
        let n = normal(t).unwrap_or([0.0; 3]);
        for v in std::iter::once(n).chain(t.iter().copied()) {
            for c in v {
                out.extend_from_slice(&(c as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

fn normal(t: &[[f64; 3]; 3]) -> Option<[f64; 3]> {
    normalize(cross(sub(t[1], t[0]), sub(t[2], t[0])))
}

/// A part's skin: its corners once each (matched by position), its triangles as corners (those
/// with no area left out), each with its colour.
struct Skin {
    points: Vec<[f64; 3]>,
    triangles: Vec<[usize; 3]>,
    colours: Vec<[u8; 3]>,
}

impl Skin {
    fn of(part: &ExportPart) -> Self {
        let mut index: HashMap<[u64; 3], usize> = HashMap::new();
        let mut points = Vec::new();
        let (mut triangles, mut colours) = (Vec::new(), Vec::new());
        for (k, t) in part.triangles.iter().enumerate() {
            if !t.iter().flatten().all(|c| c.is_finite()) || normal(t).is_none() {
                continue;
            }
            let c = t.map(|p| {
                *index.entry(p.map(f64::to_bits)).or_insert_with(|| {
                    points.push(p);
                    points.len() - 1
                })
            });
            if c[0] == c[1] || c[1] == c[2] || c[2] == c[0] {
                continue;
            }
            triangles.push(c);
            colours.push(part.colours.get(k).copied().unwrap_or(GREY));
        }
        Skin { points, triangles, colours }
    }

    /// Whether every edge is met by exactly two triangles, running opposite ways: a closed skin
    /// facing one way, which bounds a solid.
    fn closed(&self) -> bool {
        let mut edges: HashMap<(usize, usize), u32> = HashMap::new();
        for t in &self.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a, b)).or_default() += 1;
            }
        }
        !edges.is_empty() && edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
    }

    /// Six times the volume it bounds: below zero when it faces inwards.
    fn volume6(&self) -> f64 {
        self.triangles.iter().map(|t| dot(self.points[t[0]], cross(self.points[t[1]], self.points[t[2]]))).sum()
    }

    fn normal(&self, t: usize) -> Option<[f64; 3]> {
        normal(&self.triangles[t].map(|i| self.points[i]))
    }

    fn size(&self) -> f64 {
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &self.points {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        if self.points.is_empty() { 0.0 } else { length(sub(hi, lo)) }
    }

    /// The flat faces: groups of triangles in one plane, of one colour, each joined to the next
    /// across an edge only the two of them meet at.
    fn faces(&self) -> Vec<Vec<usize>> {
        let mut by_edge: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (k, t) in self.triangles.iter().enumerate() {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                by_edge.entry((a.min(b), a.max(b))).or_default().push(k);
            }
        }
        // Within a millionth of the part's size of the first triangle's plane, and facing its way.
        let tolerance = (self.size() * 1e-6).max(1e-7);
        let normals: Vec<_> = (0..self.triangles.len()).map(|t| self.normal(t)).collect();
        let mut face_of = vec![usize::MAX; self.triangles.len()];
        let mut faces = Vec::new();
        for seed in 0..self.triangles.len() {
            if face_of[seed] != usize::MAX {
                continue;
            }
            let Some(n) = normals[seed] else { continue };
            let origin = self.points[self.triangles[seed][0]];
            let id = faces.len();
            face_of[seed] = id;
            let mut face = vec![seed];
            let mut next = 0;
            while let Some(&t) = face.get(next) {
                next += 1;
                let c = self.triangles[t];
                for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
                    let Some(across) = by_edge.get(&(a.min(b), a.max(b))) else { continue };
                    if across.len() != 2 {
                        continue;
                    }
                    let other = if across[0] == t { across[1] } else { across[0] };
                    if face_of[other] != usize::MAX || self.colours[other] != self.colours[seed] {
                        continue;
                    }
                    let flat = normals[other].is_some_and(|m| dot(m, n) > 1.0 - 1e-6)
                        && self.triangles[other].iter().all(|&i| dot(sub(self.points[i], origin), n).abs() <= tolerance);
                    if flat {
                        face_of[other] = id;
                        face.push(other);
                    }
                }
            }
            faces.push(face);
        }
        faces
    }

    /// The loops round `face` (its triangles), each running round it the way its triangles do:
    /// corners in order. `None` when its edge doesn't close up.
    fn loops(&self, face: &[usize]) -> Option<Vec<Vec<usize>>> {
        let inside: HashSet<(usize, usize)> = face
            .iter()
            .flat_map(|&t| {
                let c = self.triangles[t];
                [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])]
            })
            .collect();
        let mut from: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut count = 0;
        for &(a, b) in &inside {
            if !inside.contains(&(b, a)) {
                from.entry(a).or_default().push(b);
                count += 1;
            }
        }
        // In a fixed order, so the same model makes the same file.
        let mut starts: Vec<usize> = from.keys().copied().collect();
        starts.sort_unstable();
        for to in from.values_mut() {
            to.sort_unstable_by(|a, b| b.cmp(a));
        }
        let mut loops = Vec::new();
        let mut used = 0;
        while used < count {
            let start = *starts.iter().find(|s| from.get(s).is_some_and(|to| !to.is_empty()))?;
            let mut corners = vec![start];
            let mut at = start;
            loop {
                let to = from.get_mut(&at)?.pop()?;
                used += 1;
                if to == start {
                    break;
                }
                corners.push(to);
                at = to;
                if corners.len() > count {
                    return None;
                }
            }
            loops.push(corners);
        }
        Some(loops)
    }
}

/// A STEP real: always with a point, as the format wants.
fn real(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    let s = if s.ends_with('.') { format!("{s}0") } else { s.to_owned() };
    if s == "-0.0" { "0.0".to_owned() } else { s }
}

/// Text in a STEP string: quotes doubled, plain ASCII kept, the rest as `?`.
fn text(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\'' {
                "''".to_owned()
            } else if c.is_ascii() && !c.is_ascii_control() && c != '\\' {
                c.to_string()
            } else {
                "?".to_owned()
            }
        })
        .collect()
}

/// A part's name as a product: without the copy number a model tree gives it ("Bracket<2>"), and
/// without characters a file name can't hold, since CAD programs name the part files after it.
fn product_name(name: &str) -> String {
    let trimmed = name.trim();
    let base = match trimmed.rfind('<') {
        Some(at) if trimmed.ends_with('>') && trimmed[at + 1..trimmed.len() - 1].chars().all(|c| c.is_ascii_digit()) => &trimmed[..at],
        _ => trimmed,
    };
    let safe: String = base.trim().chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    if safe.is_empty() { "Part".to_owned() } else { safe }
}

/// The DATA section being written: entities numbered as they are added.
struct Data {
    out: String,
    id: usize,
    /// Each colour's style, once.
    styles: HashMap<[u8; 3], usize>,
}

impl Data {
    fn add(&mut self, body: &str) -> usize {
        self.id += 1;
        let _ = writeln!(self.out, "#{}={body};", self.id);
        self.id
    }

    fn point(&mut self, p: [f64; 3]) -> usize {
        self.add(&format!("CARTESIAN_POINT('',({},{},{}))", real(p[0]), real(p[1]), real(p[2])))
    }

    fn direction(&mut self, d: [f64; 3]) -> usize {
        self.add(&format!("DIRECTION('',({},{},{}))", real(d[0]), real(d[1]), real(d[2])))
    }

    fn axes(&mut self) -> usize {
        let origin = self.point([0.0; 3]);
        let z = self.direction([0.0, 0.0, 1.0]);
        let x = self.direction([1.0, 0.0, 0.0]);
        self.add(&format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"))
    }

    /// The presentation style that paints a surface `colour`.
    fn style(&mut self, colour: [u8; 3]) -> usize {
        if let Some(&s) = self.styles.get(&colour) {
            return s;
        }
        let [r, g, b] = colour.map(|c| real(f64::from(c) / 255.0));
        let rgb = self.add(&format!("COLOUR_RGB('',{r},{g},{b})"));
        let fill = self.add(&format!("FILL_AREA_STYLE_COLOUR('',#{rgb})"));
        let area = self.add(&format!("FILL_AREA_STYLE('',(#{fill}))"));
        let surface = self.add(&format!("SURFACE_STYLE_FILL_AREA(#{area})"));
        let side = self.add(&format!("SURFACE_SIDE_STYLE('',(#{surface}))"));
        let usage = self.add(&format!("SURFACE_STYLE_USAGE(.BOTH.,#{side})"));
        let style = self.add(&format!("PRESENTATION_STYLE_ASSIGNMENT((#{usage}))"));
        self.styles.insert(colour, style);
        style
    }
}

/// The body made of a part: its representation and what to colour.
struct Body {
    representation: usize,
    /// The body and its colour, then each face of another colour with its own.
    styled: Vec<(usize, [u8; 3])>,
}

/// The pieces of a face to write: its triangles and the loops round them.
type Pieces = Vec<(Vec<usize>, Vec<Vec<usize>>)>;

/// Twice the area `corners` run round, looking down `n` (below zero when they run clockwise).
fn area(skin: &Skin, corners: &[usize], n: [f64; 3]) -> f64 {
    let mut a = [0.0; 3];
    for k in 0..corners.len() {
        let c = cross(skin.points[corners[k]], skin.points[corners[(k + 1) % corners.len()]]);
        a = [a[0] + c[0], a[1] + c[1], a[2] + c[2]];
    }
    dot(a, n)
}

/// Writes the edges, vertices and loops of one face; its bounds.
fn bounds(d: &mut Data, skin: &Skin, loops: &[Vec<usize>], outer: usize, edges: &mut HashMap<(usize, usize), usize>, vertices: &mut HashMap<usize, usize>) -> Vec<String> {
    let mut out = Vec::with_capacity(loops.len());
    for (k, l) in loops.iter().enumerate() {
        let mut oriented = Vec::with_capacity(l.len());
        for j in 0..l.len() {
            let (a, b) = (l[j], l[(j + 1) % l.len()]);
            let key = (a.min(b), a.max(b));
            let edge = match edges.get(&key) {
                Some(&e) => e,
                None => {
                    let mut vertex = |d: &mut Data, i: usize| {
                        *vertices.entry(i).or_insert_with(|| {
                            let p = d.point(skin.points[i]);
                            d.add(&format!("VERTEX_POINT('',#{p})"))
                        })
                    };
                    let va = vertex(d, key.0);
                    let vb = vertex(d, key.1);
                    let start = d.point(skin.points[key.0]);
                    let along = sub(skin.points[key.1], skin.points[key.0]);
                    let dir = d.direction(normalize(along).unwrap_or([1.0, 0.0, 0.0]));
                    let vector = d.add(&format!("VECTOR('',#{dir},{})", real(length(along))));
                    let line = d.add(&format!("LINE('',#{start},#{vector})"));
                    let e = d.add(&format!("EDGE_CURVE('',#{va},#{vb},#{line},.T.)"));
                    edges.insert(key, e);
                    e
                }
            };
            let same = if a < b { ".T." } else { ".F." };
            oriented.push(format!("#{}", d.add(&format!("ORIENTED_EDGE('',*,*,#{edge},{same})"))));
        }
        let lp = d.add(&format!("EDGE_LOOP('',({}))", oriented.join(",")));
        let kind = if k == outer { "FACE_OUTER_BOUND" } else { "FACE_BOUND" };
        out.push(format!("#{}", d.add(&format!("{kind}('',#{lp},.T.)"))));
    }
    out
}

/// `part` as a solid when its skin is closed, else a surface body, its flat faces merged.
fn body(d: &mut Data, part: &ExportPart, name: &str, ctx: usize) -> Option<Body> {
    let mut skin = Skin::of(part);
    if skin.triangles.is_empty() {
        return None;
    }
    let solid = skin.closed();
    if solid && skin.volume6() < 0.0 {
        for t in &mut skin.triangles {
            t.swap(1, 2);
        }
    }
    // The part's colour: that of most of its triangles.
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for c in &skin.colours {
        *counts.entry(*c).or_default() += 1;
    }
    let main = counts.into_iter().max_by_key(|&(c, n)| (n, c)).map_or(GREY, |(c, _)| c);
    let mut vertices: HashMap<usize, usize> = HashMap::new();
    let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
    let mut faces = Vec::new();
    let mut odd = Vec::new();
    for face in skin.faces() {
        // A face whose edge doesn't close up is written as its triangles.
        let pieces: Pieces = match skin.loops(&face) {
            Some(loops) => vec![(face.clone(), loops)],
            None => face.iter().map(|&t| (vec![t], vec![skin.triangles[t].to_vec()])).collect(),
        };
        for (triangles, loops) in pieces {
            let Some(n) = skin.normal(triangles[0]) else { continue };
            // The outer loop: the one round the most area.
            let outer = (0..loops.len()).max_by(|&a, &b| area(&skin, &loops[a], n).total_cmp(&area(&skin, &loops[b], n))).unwrap_or(0);
            let bounds = bounds(d, &skin, &loops, outer, &mut edges, &mut vertices);
            let first = loops[outer][0];
            let x = loops[outer].get(1).and_then(|&p| {
                let along = sub(skin.points[p], skin.points[first]);
                normalize(sub(along, scale(n, dot(along, n))))
            });
            let x = x.or_else(|| normalize(cross(n, [0.0, 0.0, 1.0]))).or_else(|| normalize(cross(n, [1.0, 0.0, 0.0]))).unwrap_or([1.0, 0.0, 0.0]);
            let origin = d.point(skin.points[first]);
            let nz = d.direction(n);
            let nx = d.direction(x);
            let place = d.add(&format!("AXIS2_PLACEMENT_3D('',#{origin},#{nz},#{nx})"));
            let plane = d.add(&format!("PLANE('',#{place})"));
            let f = d.add(&format!("ADVANCED_FACE('',({}),#{plane},.T.)", bounds.join(",")));
            faces.push(format!("#{f}"));
            let colour = skin.colours[triangles[0]];
            if colour != main {
                odd.push((f, colour));
            }
        }
    }
    let name = text(name);
    let axes = d.axes();
    let (item, representation) = if solid {
        let shell = d.add(&format!("CLOSED_SHELL('',({}))", faces.join(",")));
        let brep = d.add(&format!("MANIFOLD_SOLID_BREP('{name}',#{shell})"));
        (brep, d.add(&format!("ADVANCED_BREP_SHAPE_REPRESENTATION('{name}',(#{axes},#{brep}),#{ctx})")))
    } else {
        let shell = d.add(&format!("OPEN_SHELL('',({}))", faces.join(",")));
        let model = d.add(&format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))"));
        (model, d.add(&format!("MANIFOLD_SURFACE_SHAPE_REPRESENTATION('{name}',(#{axes},#{model}),#{ctx})")))
    };
    let mut styled = vec![(item, main)];
    styled.extend(odd);
    Some(Body { representation, styled })
}

/// A product and its definition: (its PRODUCT_DEFINITION, its PRODUCT_DEFINITION_SHAPE).
fn product(d: &mut Data, name: &str, product_ctx: usize, def_ctx: usize) -> (usize, usize) {
    let name = text(name);
    let product = d.add(&format!("PRODUCT('{name}','{name}','',(#{product_ctx}))"));
    d.add(&format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part','',(#{product}))"));
    let formation = d.add(&format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let definition = d.add(&format!("PRODUCT_DEFINITION('design','',#{formation},#{def_ctx})"));
    let shape = d.add(&format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
    (definition, shape)
}

/// The parts as one STEP file (AP214): one part as a part, several as an assembly of them.
#[must_use]
pub fn step(title: &str, date: &str, parts: &[ExportPart]) -> String {
    let mut d = Data { out: String::new(), id: 0, styles: HashMap::new() };
    let app = d.add("APPLICATION_CONTEXT('automotive design')");
    d.add(&format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));
    let product_ctx = d.add(&format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let def_ctx = d.add(&format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
    let mm = d.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))");
    let rad = d.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
    let sr = d.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
    let tol = d.add(&format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-05),#{mm},'distance_accuracy_value','')"));
    let ctx = d.add(&format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{tol}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr}))REPRESENTATION_CONTEXT('',''))"
    ));
    // Names once each: a second "Bracket" is "Bracket (2)".
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut made = Vec::new();
    for part in parts {
        let base = product_name(&part.name);
        let n = seen.entry(base.to_lowercase()).or_default();
        *n += 1;
        let name = if *n == 1 { base } else { format!("{base} ({n})") };
        if let Some(b) = body(&mut d, part, &name, ctx) {
            let (definition, shape) = product(&mut d, &name, product_ctx, def_ctx);
            d.add(&format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{})", b.representation));
            made.push((definition, b));
        }
    }
    // Colours.
    let mut styled = Vec::new();
    for (_, b) in &made {
        for &(item, colour) in &b.styled {
            let style = d.style(colour);
            styled.push(format!("#{}", d.add(&format!("STYLED_ITEM('color',(#{style}),#{item})"))));
        }
    }
    if !styled.is_empty() {
        d.add(&format!("MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{ctx})", styled.join(",")));
    }
    // Several parts: an assembly of them, each where the model places it.
    if made.len() > 1 {
        let name = product_name(title);
        let (root, root_shape) = product(&mut d, &name, product_ctx, def_ctx);
        let root_axes = d.axes();
        let root_rep = d.add(&format!("SHAPE_REPRESENTATION('{}',(#{root_axes}),#{ctx})", text(&name)));
        d.add(&format!("SHAPE_DEFINITION_REPRESENTATION(#{root_shape},#{root_rep})"));
        for (k, (definition, b)) in made.iter().enumerate() {
            let n = k + 1;
            let usage = d.add(&format!("NEXT_ASSEMBLY_USAGE_OCCURRENCE('{n}','','',#{root},#{definition},$)"));
            let usage_shape = d.add(&format!("PRODUCT_DEFINITION_SHAPE('','',#{usage})"));
            let part_axes = d.axes();
            let moved = d.add(&format!("ITEM_DEFINED_TRANSFORMATION('','',#{part_axes},#{root_axes})"));
            let relation = d.add(&format!(
                "(REPRESENTATION_RELATIONSHIP('','',#{},#{root_rep})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{moved})SHAPE_REPRESENTATION_RELATIONSHIP())",
                b.representation
            ));
            d.add(&format!("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{relation},#{usage_shape})"));
        }
    }
    let title = text(title);
    let mut s = String::with_capacity(d.out.len() + 512);
    let _ = writeln!(s, "ISO-10303-21;");
    let _ = writeln!(s, "HEADER;");
    let _ = writeln!(s, "FILE_DESCRIPTION(('{title}: from a 3D PDF'),'2;1');");
    let _ = writeln!(s, "FILE_NAME('{title}','{}',(''),(''),'pdfcraft','pdfcraft','');", text(date));
    let _ = writeln!(s, "FILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));");
    let _ = writeln!(s, "ENDSEC;");
    let _ = writeln!(s, "DATA;");
    s.push_str(&d.out);
    let _ = writeln!(s, "ENDSEC;");
    let _ = writeln!(s, "END-ISO-10303-21;");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube's twelve triangles, outward.
    fn cube() -> Vec<[[f64; 3]; 3]> {
        let p = |i: u8| [f64::from(i & 1), f64::from((i >> 1) & 1), f64::from((i >> 2) & 1)];
        let quads: [[u8; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        quads.iter().flat_map(|q| [[p(q[0]), p(q[1]), p(q[2])], [p(q[0]), p(q[2]), p(q[3])]]).collect()
    }

    fn part(name: &str, triangles: Vec<[[f64; 3]; 3]>) -> ExportPart {
        let colours = vec![[200, 30, 30]; triangles.len()];
        ExportPart { name: name.into(), triangles, colours }
    }

    #[test]
    fn stl_holds_every_triangle_with_its_normal() {
        let b = stl("solid cube", &[part("Cube", cube())]);
        assert_eq!(b.len(), 84 + 12 * 50);
        assert_eq!(&b[..5], b"pdfcr");
        assert_eq!(u32::from_le_bytes([b[80], b[81], b[82], b[83]]), 12);
    }

    #[test]
    fn a_closed_part_is_a_solid_of_flat_faces_and_an_open_one_a_surface() {
        assert!(Skin::of(&part("Cube", cube())).closed());
        let open: Vec<_> = cube().into_iter().skip(2).collect();
        assert!(!Skin::of(&part("Lid", open.clone())).closed());
        let s = step("Box", "2026-11-18", &[part("Cube 'A'<1>", cube()), part("Lid", open)]);
        assert!(s.starts_with("ISO-10303-21;") && s.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(s.contains("MANIFOLD_SOLID_BREP('Cube ''A''',") && s.contains("SHELL_BASED_SURFACE_MODEL('Lid'"));
        // The cube's two triangles a side make one square face: six faces, then the lid's five.
        assert_eq!(s.matches("ADVANCED_FACE").count(), 6 + 5);
        assert_eq!(s.matches("EDGE_CURVE").count(), 12 + 12);
        // Coloured, and an assembly of the two.
        assert!(s.contains("COLOUR_RGB('',0.784314,0.117647,0.117647)") && s.contains("STYLED_ITEM('color'"));
        assert_eq!(s.matches("NEXT_ASSEMBLY_USAGE_OCCURRENCE").count(), 2);
        assert_eq!(real(1.0), "1.0");
        assert_eq!(real(-0.25), "-0.25");
    }

    #[test]
    fn an_inside_out_solid_is_turned_and_a_flat_face_keeps_its_hole() {
        // A cube facing in is written facing out.
        let inside_out: Vec<_> = cube().into_iter().map(|[a, b, c]| [a, c, b]).collect();
        let mut skin = Skin::of(&part("In", inside_out));
        assert!(skin.closed() && skin.volume6() < 0.0);
        for t in &mut skin.triangles {
            t.swap(1, 2);
        }
        assert!(skin.volume6() > 0.0);
        // A square with a square hole, in eight triangles: one face of two loops.
        let p = |x: f64, y: f64| [x, y, 0.0];
        let (o, i) = ([p(0.0, 0.0), p(3.0, 0.0), p(3.0, 3.0), p(0.0, 3.0)], [p(1.0, 1.0), p(2.0, 1.0), p(2.0, 2.0), p(1.0, 2.0)]);
        let mut ring = Vec::new();
        for k in 0..4 {
            let (a, b, c, d) = (o[k], o[(k + 1) % 4], i[(k + 1) % 4], i[k]);
            ring.push([a, b, c]);
            ring.push([a, c, d]);
        }
        let skin = Skin::of(&part("Ring", ring));
        let faces = skin.faces();
        assert_eq!(faces.len(), 1);
        assert_eq!(skin.loops(&faces[0]).unwrap().len(), 2);
        assert_eq!(product_name("Bracket<12>"), "Bracket");
        assert_eq!(product_name("a/b"), "a_b");
    }
}
