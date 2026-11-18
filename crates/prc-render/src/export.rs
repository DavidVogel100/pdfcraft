//! Parts written out as files other programs read: binary STL (for 3D printing) and STEP
//! (ISO 10303-21, AP214) made of facets, since a 3D PDF carries the parts' triangles, not their
//! CAD solids. A closed part becomes a faceted solid (`FACETED_BREP`), any other a surface body
//! (`SHELL_BASED_SURFACE_MODEL`). Coordinates are written as given, in millimetres.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::vec3::{cross, normalize, sub};

/// One part to write: its name and its triangles, in millimetres.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportPart {
    pub name: String,
    pub triangles: Vec<[[f64; 3]; 3]>,
}

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

/// Whether every edge of `triangles` (corners matched by position) is shared by exactly two of
/// them: a closed skin, which bounds a solid.
fn closed(triangles: &[[[f64; 3]; 3]]) -> bool {
    let key = |p: [f64; 3]| p.map(f64::to_bits);
    let mut edges: HashMap<([u64; 3], [u64; 3]), u32> = HashMap::new();
    for t in triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (a, b) = (key(a), key(b));
            *edges.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
        }
    }
    !edges.is_empty() && edges.values().all(|&n| n == 2)
}

/// A STEP real: always with a point, as the format wants.
fn real(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') { format!("{s}0") } else { s.to_owned() }
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

/// The parts as one STEP file (AP214), each part a product of its own.
#[must_use]
pub fn step(title: &str, date: &str, parts: &[ExportPart]) -> String {
    let mut s = String::new();
    let mut id = 0usize;
    let mut next = |line: &mut String, body: &str| -> usize {
        id += 1;
        let _ = writeln!(line, "#{id}={body};");
        id
    };
    let mut d = String::new();
    let app = next(&mut d, "APPLICATION_CONTEXT('automotive design')");
    next(&mut d, &format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));
    let product_ctx = next(&mut d, &format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let def_ctx = next(&mut d, &format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
    let mm = next(&mut d, "(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))");
    let rad = next(&mut d, "(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
    let sr = next(&mut d, "(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
    let tol = next(&mut d, &format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-05),#{mm},'distance_accuracy_value','')"));
    let ctx = next(
        &mut d,
        &format!(
            "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{tol}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr}))REPRESENTATION_CONTEXT('',''))"
        ),
    );
    let origin = next(&mut d, "CARTESIAN_POINT('',(0.,0.,0.))");
    let z = next(&mut d, "DIRECTION('',(0.,0.,1.))");
    let x = next(&mut d, "DIRECTION('',(1.,0.,0.))");
    let axes = next(&mut d, &format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"));
    for part in parts.iter().filter(|p| !p.triangles.is_empty()) {
        let name = text(&part.name);
        let product = next(&mut d, &format!("PRODUCT('{name}','{name}','',(#{product_ctx}))"));
        next(&mut d, &format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part','',(#{product}))"));
        let formation = next(&mut d, &format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
        let definition = next(&mut d, &format!("PRODUCT_DEFINITION('design','',#{formation},#{def_ctx})"));
        let shape = next(&mut d, &format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
        // Each corner once.
        let mut points: HashMap<[u64; 3], usize> = HashMap::new();
        let mut point = |d: &mut String, next: &mut dyn FnMut(&mut String, &str) -> usize, p: [f64; 3]| -> usize {
            *points
                .entry(p.map(f64::to_bits))
                .or_insert_with(|| next(d, &format!("CARTESIAN_POINT('',({},{},{}))", real(p[0]), real(p[1]), real(p[2]))))
        };
        let mut faces = Vec::with_capacity(part.triangles.len());
        for t in &part.triangles {
            let Some(n) = normal(t) else { continue };
            let Some(along) = normalize(sub(t[1], t[0])) else { continue };
            let [a, b, c] = t.map(|p| point(&mut d, &mut next, p));
            let lp = next(&mut d, &format!("POLY_LOOP('',(#{a},#{b},#{c}))"));
            let bound = next(&mut d, &format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
            let nz = next(&mut d, &format!("DIRECTION('',({},{},{}))", real(n[0]), real(n[1]), real(n[2])));
            let nx = next(&mut d, &format!("DIRECTION('',({},{},{}))", real(along[0]), real(along[1]), real(along[2])));
            let place = next(&mut d, &format!("AXIS2_PLACEMENT_3D('',#{a},#{nz},#{nx})"));
            let plane = next(&mut d, &format!("PLANE('',#{place})"));
            faces.push(next(&mut d, &format!("FACE_SURFACE('',(#{bound}),#{plane},.T.)")));
        }
        let list = faces.iter().map(|f| format!("#{f}")).collect::<Vec<_>>().join(",");
        let representation = if closed(&part.triangles) {
            let shell = next(&mut d, &format!("CLOSED_SHELL('',({list}))"));
            let brep = next(&mut d, &format!("FACETED_BREP('{name}',#{shell})"));
            next(&mut d, &format!("FACETED_BREP_SHAPE_REPRESENTATION('{name}',(#{brep},#{axes}),#{ctx})"))
        } else {
            let shell = next(&mut d, &format!("OPEN_SHELL('',({list}))"));
            let model = next(&mut d, &format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))"));
            next(&mut d, &format!("MANIFOLD_SURFACE_SHAPE_REPRESENTATION('{name}',(#{model},#{axes}),#{ctx})"))
        };
        next(&mut d, &format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{representation})"));
    }
    let title = text(title);
    let _ = writeln!(s, "ISO-10303-21;");
    let _ = writeln!(s, "HEADER;");
    let _ = writeln!(s, "FILE_DESCRIPTION(('{title}: facets from a 3D PDF'),'2;1');");
    let _ = writeln!(s, "FILE_NAME('{title}','{}',(''),(''),'pdfcraft','pdfcraft','');", text(date));
    let _ = writeln!(s, "FILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));");
    let _ = writeln!(s, "ENDSEC;");
    let _ = writeln!(s, "DATA;");
    s.push_str(&d);
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

    #[test]
    fn stl_holds_every_triangle_with_its_normal() {
        let parts = [ExportPart { name: "Cube".into(), triangles: cube() }];
        let b = stl("solid cube", &parts);
        assert_eq!(b.len(), 84 + 12 * 50);
        assert_eq!(&b[..5], b"pdfcr");
        assert_eq!(u32::from_le_bytes([b[80], b[81], b[82], b[83]]), 12);
    }

    #[test]
    fn a_closed_part_is_a_faceted_solid_and_an_open_one_a_surface() {
        assert!(closed(&cube()));
        let open: Vec<_> = cube().into_iter().skip(2).collect();
        assert!(!closed(&open));
        let s = step(
            "Box",
            "2026-11-18",
            &[ExportPart { name: "Cube 'A'".into(), triangles: cube() }, ExportPart { name: "Lid".into(), triangles: open }],
        );
        assert!(s.starts_with("ISO-10303-21;") && s.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(s.contains("FACETED_BREP('Cube ''A''',") && s.contains("SHELL_BASED_SURFACE_MODEL('Lid'"));
        // The cube's eight corners once each, with the origin's own point.
        assert_eq!(s.matches("CARTESIAN_POINT").count(), 1 + 8 + 8);
        assert_eq!(s.matches("FACE_SURFACE").count(), 12 + 10);
        assert_eq!(real(1.0), "1.0");
        assert_eq!(real(-0.25), "-0.25");
    }
}
