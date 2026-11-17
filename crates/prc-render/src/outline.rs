//! The edges a CAD program draws on a shaded model: where one face of a part meets another,
//! sharp or tangent (where a fillet starts on a flat wall), and the open edges of a face.

use std::collections::HashMap;

use craft_prc::TriangleMesh;

/// An edge by its corners' positions (their bits), in order.
type EdgeKey = [[u64; 3]; 2];

/// What [`outline_edges`] knows of an edge: the face that drew it first, whether another face
/// did too, how many triangles did, and the corner indices first seen.
type EdgeUse = (usize, bool, u32, [u32; 2]);

/// The edges of `mesh` between its faces ([`TriangleMesh::faces`]), and those only one triangle
/// has (an open edge), as pairs of indices into its positions; each once. A mesh without faces
/// gives only its open edges. Corners are matched by position, as a producer may write a corner
/// once per face.
#[must_use]
pub fn outline_edges(mesh: &TriangleMesh) -> Vec<[u32; 2]> {
    let key = |i: u32| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i)).map(|p| p.map(f64::to_bits));
    let mut seen: HashMap<EdgeKey, EdgeUse> = HashMap::new();
    let whole: Vec<std::ops::Range<usize>> = std::iter::once(0..mesh.triangles.len()).collect();
    let faces: &[std::ops::Range<usize>] = if mesh.faces.is_empty() { &whole } else { &mesh.faces };
    for (face, range) in faces.iter().enumerate() {
        for tri in mesh.triangles.get(range.clone()).unwrap_or(&[]) {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let (Some(ka), Some(kb)) = (key(a), key(b)) else { continue };
                if ka == kb {
                    continue;
                }
                let k = if ka < kb { [ka, kb] } else { [kb, ka] };
                let entry = seen.entry(k).or_insert((face, false, 0, [a, b]));
                entry.1 |= entry.0 != face;
                entry.2 = entry.2.saturating_add(1);
            }
        }
    }
    let mut out: Vec<[u32; 2]> = seen.into_values().filter(|&(_, between, count, _)| between || count == 1).map(|(.., e)| e).collect();
    // In a steady order, whatever the hashing.
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two squares side by side sharing an edge, each two triangles; `split` puts them in
    /// separate faces.
    fn two_squares(split: bool) -> TriangleMesh {
        let mut m = TriangleMesh::default();
        m.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 0.0]];
        m.triangles = vec![[0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4]];
        m.faces = if split { vec![0..2, 2..4] } else { Vec::new() };
        m
    }

    #[test]
    fn the_edge_where_two_faces_meet_is_drawn_and_one_inside_a_face_is_not() {
        let edges = outline_edges(&two_squares(true));
        // The square's six outer edges, and the shared edge 1-4; not the diagonals.
        assert_eq!(edges.len(), 7, "{edges:?}");
        assert!(edges.iter().any(|e| *e == [1, 4] || *e == [4, 1]));
        assert!(!edges.iter().any(|e| *e == [0, 4] || *e == [4, 0]), "a diagonal inside a face");
        // As one face, the shared edge is inside it.
        assert_eq!(outline_edges(&two_squares(false)).len(), 6);
    }

    #[test]
    fn corners_written_once_per_face_still_meet() {
        // The right square's own copies of the shared corners.
        let mut m = two_squares(true);
        m.positions.extend([[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
        m.triangles[2] = [6, 2, 5];
        m.triangles[3] = [6, 5, 7];
        assert_eq!(outline_edges(&m).len(), 7);
    }
}
