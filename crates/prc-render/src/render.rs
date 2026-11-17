//! The camera, the render options and [`render`]: world meshes to an RGBA image.
//!
//! One sample per pixel centre, no antialiasing. Faces are lit on both sides because a
//! producer's winding is not trusted to face outward.

use crate::WorldMesh;
use crate::raster::{NO_ID, Target, View, ViewCorner, at, channel, pixel_index};
use crate::vec3::{cross, dot, length, normalize, scale, span, sub, unit_cross};
use craft_prc::TriangleMesh;
use std::ops::Range;
use std::sync::atomic::AtomicU64;

/// The largest image [`render`] draws, in pixels (width × height).
pub const MAX_RENDER_PIXELS: u64 = 16 * 1024 * 1024;

/// The most pixel centres one [`render`] tests, summed over each drawn triangle's bounding box
/// clipped to the image: 128 full layers of the largest image.
pub const MAX_RENDER_WORK: u64 = 1 << 31;

/// [`Camera::fit`]'s framed extent over the model's: 5% clear on each side.
const FIT_MARGIN: f64 = 1.1;

/// How the camera projects the scene onto the image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// A pinhole camera with this vertical field of view, in degrees (exclusive range 0–180).
    Perspective {
        /// Vertical field of view, degrees.
        fov_y: f64,
    },
    /// A parallel projection showing this much of the scene vertically, in model units.
    Orthographic {
        /// Visible height, model units.
        height: f64,
    },
}

/// Which side of the annotation a PDF 3D view's projection is relative to (`/PS`, `/OB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSide {
    /// The width.
    Width,
    /// The height.
    Height,
    /// The shorter side.
    Min,
    /// The longer side.
    Max,
}

/// A PDF 3D view's projection (`/P`, ISO 32000-2 §13.6.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewLens {
    /// A field of view, degrees, across `side`.
    Perspective {
        /// Field of view, degrees (`/FOV`).
        fov: f64,
        /// The side it spans (`/PS`).
        side: ViewSide,
    },
    /// Parallel: `1 / scale` model units span `side`.
    Orthographic {
        /// `/OS`.
        scale: f64,
        /// `/OB`.
        side: ViewSide,
    },
}

/// Where the camera is and where it looks, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The camera position.
    pub eye: [f64; 3],
    /// The point at the image centre.
    pub target: [f64; 3],
    /// The direction that appears upward; must not be parallel to the view direction.
    pub up: [f64; 3],
    /// The projection.
    pub projection: Projection,
}

/// An axis-aligned box around every point a set of meshes draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// The smallest x, y and z.
    pub min: [f64; 3],
    /// The largest x, y and z.
    pub max: [f64; 3],
}

impl Bounds {
    /// The box around every finite vertex the meshes' drawn triangles use, or `None` when there
    /// is none.
    #[must_use]
    pub fn of(meshes: &[WorldMesh]) -> Option<Bounds> {
        let mut out: Option<Bounds> = None;
        for p in vertices(meshes) {
            let b = out.get_or_insert(Bounds { min: p, max: p });
            for ((lo, hi), c) in b.min.iter_mut().zip(b.max.iter_mut()).zip(p) {
                *lo = lo.min(c);
                *hi = hi.max(c);
            }
        }
        out
    }

    /// The box's centre, finite for any finite box.
    #[must_use]
    pub fn centre(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| at(self.min, i) / 2.0 + at(self.max, i) / 2.0)
    }

    /// Half the box's diagonal: the radius of a sphere holding it.
    #[must_use]
    pub fn radius(&self) -> f64 {
        length(sub(self.max, self.min)) / 2.0
    }
}

/// Every corner of the triangles [`render`] draws: those not fully transparent whose three
/// corners exist and are finite.
pub(crate) fn vertices(meshes: &[WorldMesh]) -> impl Iterator<Item = [f64; 3]> + Clone + '_ {
    meshes
        .iter()
        .flat_map(|m| m.mesh.triangles.iter().enumerate().filter(|&(t, _)| !invisible(m, t)).filter_map(|(_, tri)| corners(&m.mesh, tri)).flatten())
}

/// Whether triangle `t` of `m` is fully transparent, so [`render`] does not draw it.
fn invisible(m: &WorldMesh, t: usize) -> bool {
    m.colour_of(t).is_some_and(|c| c[3] == 0)
}

impl Camera {
    /// A camera looking along `direction` that fits `bounds` in an image of the given aspect
    /// ratio (width / height), with a 30° perspective or an orthographic projection: the box's
    /// eight corners framed from this direction, about 5% clear on each side of the tighter axis.
    ///
    /// # Errors
    /// [`RenderError::Camera`] when `direction` is zero or parallel to `up`, or `aspect` is not positive.
    pub fn fit(bounds: &Bounds, direction: [f64; 3], up: [f64; 3], perspective: bool, aspect: f64) -> Result<Camera, RenderError> {
        let corners = (0..8).map(|k| [0, 1, 2].map(|i| if k >> i & 1 == 0 { at(bounds.min, i) } else { at(bounds.max, i) }));
        Camera::frame(bounds, corners, direction, up, (perspective, aspect))
    }

    /// As [`Camera::fit`], framing every vertex the meshes' drawn triangles use rather than their box,
    /// so an oblique view is not padded by empty box corners.
    ///
    /// # Errors
    /// As [`Camera::fit`], and when the meshes have no finite vertex.
    pub fn fit_meshes(meshes: &[WorldMesh], direction: [f64; 3], up: [f64; 3], perspective: bool, aspect: f64) -> Result<Camera, RenderError> {
        let bounds = Bounds::of(meshes).ok_or(RenderError::Camera("the model has no vertex"))?;
        Camera::frame(&bounds, vertices(meshes), direction, up, (perspective, aspect))
    }

    /// The camera of a PDF 3D view (ISO 32000-2 §13.6.4): `camera_to_world` (`/C2W`) is the
    /// camera's x, y and z axes and its position, the camera looking along z with y up;
    /// `center_of_orbit` (`/CO`) is how far ahead it looks, or 0 when the view gives none, and then
    /// the camera looks as far as `fallback` lies along its z axis. `size` is the annotation's width
    /// and height (any unit), which the lens's field of view or scale is relative to.
    ///
    /// # Errors
    /// [`RenderError::Camera`] when the axes are zero, not finite, or the y axis lies along z.
    pub fn from_view(
        camera_to_world: [f64; 12],
        center_of_orbit: f64,
        lens: ViewLens,
        size: [f64; 2],
        fallback: [f64; 3],
    ) -> Result<Camera, RenderError> {
        let c = camera_to_world;
        if c.iter().any(|v| !v.is_finite()) {
            return Err(RenderError::Camera("the view's matrix is not finite"));
        }
        let ahead = normalize([c[6], c[7], c[8]]).ok_or(RenderError::Camera("the view's z axis is zero"))?;
        let up = normalize([c[3], c[4], c[5]]).ok_or(RenderError::Camera("the view's y axis is zero"))?;
        let eye = [c[9], c[10], c[11]];
        // The target sets the perspective near plane (1/10,000 of its distance), so it is the
        // nearer of the centre of orbit and the model's centre ahead: a far /CO must not clip
        // a model close to the eye.
        let usable = |d: f64| Some(d).filter(|d| d.is_finite() && *d > 0.0);
        let distance = match (usable(center_of_orbit), usable(dot(sub(fallback, eye), ahead))) {
            (Some(orbit), Some(model)) => orbit.min(model),
            (orbit, model) => orbit.or(model).unwrap_or(1.0),
        };
        let target = [0, 1, 2].map(|i| at(eye, i) + at(ahead, i) * distance);
        let [w, h] = size.map(|v| if v.is_finite() && v > 0.0 { v } else { 1.0 });
        let side = |s: ViewSide| match s {
            ViewSide::Width => w,
            ViewSide::Height => h,
            ViewSide::Min => w.min(h),
            ViewSide::Max => w.max(h),
        };
        let projection = match lens {
            // `1 / scale` model units span the side; the projection is given by the height.
            ViewLens::Orthographic { scale, side: s } => {
                let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
                Projection::Orthographic { height: h / side(s) / scale }
            }
            ViewLens::Perspective { fov, side: s } => {
                let half = (if fov.is_finite() && fov > 0.0 && fov < 180.0 { fov } else { 90.0 }).to_radians() / 2.0;
                Projection::Perspective { fov_y: 2.0 * (half.tan() * h / side(s)).atan().to_degrees() }
            }
        };
        let camera = Camera { eye, target, up, projection };
        camera.basis()?;
        Ok(camera)
    }

    /// Frames `points` (which `bounds` holds) from `direction`, aimed at the middle of their projection.
    fn frame(
        bounds: &Bounds,
        points: impl Iterator<Item = [f64; 3]> + Clone,
        direction: [f64; 3],
        up: [f64; 3],
        (perspective, aspect): (bool, f64),
    ) -> Result<Camera, RenderError> {
        if !(aspect.is_finite() && aspect > 0.0) {
            return Err(RenderError::Camera("the aspect ratio is not positive"));
        }
        let dir = normalize(direction).ok_or(RenderError::Camera("the view direction is zero"))?;
        let centre = bounds.centre();
        let radius = match bounds.radius() {
            r if r.is_finite() && r > 0.0 => r,
            _ => 1.0,
        };
        let [right, upward, forward] = axes(dir, up)?;
        let view = |p: [f64; 3]| {
            let r = sub(p, centre);
            [dot(r, right), dot(r, upward), dot(r, forward)]
        };
        let ((x0, x1), (y0, y1)) = points
            .clone()
            .map(view)
            .fold(((f64::INFINITY, f64::NEG_INFINITY), (f64::INFINITY, f64::NEG_INFINITY)), |((a0, a1), (b0, b1)), [x, y, _]| {
                ((a0.min(x), a1.max(x)), (b0.min(y), b1.max(y)))
            });
        if !(x0 <= x1 && y0 <= y1) {
            return Err(RenderError::Camera("the model has no vertex"));
        }
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let target = [0, 1, 2].map(|i| at(centre, i) + mx * at(right, i) + my * at(upward, i));
        let floor = radius * 1e-3;
        let (projection, distance) = if perspective {
            let fov_y: f64 = 30.0;
            let ty = (fov_y.to_radians() / 2.0).tan() / FIT_MARGIN;
            let tx = ty * aspect;
            // A point at lateral (x, y) from the target and depth f past it is in view when
            // |x| <= (d + f)·tx and |y| <= (d + f)·ty.
            let d = points.map(view).fold(floor, |d, [x, y, f]| {
                let need = ((x - mx).abs() / tx).max((y - my).abs() / ty);
                d.max(need - f).max(floor - f)
            });
            (Projection::Perspective { fov_y }, d)
        } else {
            let height = ((y1 - y0).max((x1 - x0) / aspect) * FIT_MARGIN).max(floor);
            (Projection::Orthographic { height }, 2.0 * radius)
        };
        let camera = Camera { eye: sub(target, scale(dir, distance)), target, up, projection };
        camera.basis()?;
        Ok(camera)
    }

    /// Where world point `p` falls on a `width` × `height` picture this camera draws: pixel x
    /// and y from the top left, as [`render`] places it. `None` when it is behind the eye or
    /// nearer than a perspective camera's near plane, or the camera cannot form a view.
    #[must_use]
    pub fn project(&self, p: [f64; 3], width: u32, height: u32) -> Option<[f64; 2]> {
        let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
        let view = View::new(self, w / h).ok()?;
        let [right, up, forward] = self.basis().ok()?;
        let d = sub(p, self.eye);
        let z = dot(d, forward);
        if let View::Perspective { near, .. } = view
            && z < near
        {
            return None;
        }
        let at = view.project([dot(d, right), dot(d, up), z, 0.0, 0.0, 0.0, 0.0], width.max(1), height.max(1)).at;
        (at[0].is_finite() && at[1].is_finite()).then_some([at[0], at[1]])
    }

    /// The world point at pixel (`x`, `y`) of a `width` × `height` picture this camera draws,
    /// `depth` model units ahead of the eye along the view: the inverse of [`Camera::project`]
    /// for a point at that depth. `None` when the camera cannot form a view.
    #[must_use]
    pub fn unproject(&self, x: f64, y: f64, width: u32, height: u32, depth: f64) -> Option<[f64; 3]> {
        let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
        let [right, up, forward] = self.basis().ok()?;
        let (nx, ny) = (x / w * 2.0 - 1.0, 1.0 - y / h * 2.0);
        let aspect = w / h;
        let (vx, vy) = match self.projection {
            Projection::Perspective { fov_y } => {
                let focal = 1.0 / (fov_y.to_radians() / 2.0).tan();
                (nx * depth * aspect / focal, ny * depth / focal)
            }
            Projection::Orthographic { height } => (nx * height / 2.0 * aspect, ny * height / 2.0),
        };
        let p = [0, 1, 2].map(|k| at(self.eye, k) + at(right, k) * vx + at(up, k) * vy + at(forward, k) * depth);
        p.iter().all(|c| c.is_finite()).then_some(p)
    }

    /// Right, up and forward unit vectors.
    pub(crate) fn basis(&self) -> Result<[[f64; 3]; 3], RenderError> {
        let forward = normalize(sub(self.target, self.eye)).ok_or(RenderError::Camera("the eye and the target coincide"))?;
        axes(forward, self.up)
    }
}

/// Right, up and `forward` (a unit vector) for a camera whose up direction is `up`.
fn axes(forward: [f64; 3], up: [f64; 3]) -> Result<[[f64; 3]; 3], RenderError> {
    let right = normalize(cross(forward, up)).ok_or(RenderError::Camera("the up direction is zero or parallel to the view direction"))?;
    Ok([right, cross(right, forward), forward])
}

/// How surfaces are lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Lighting {
    /// One light at the camera: a surface's colour times `0.3 + 0.7·|n·v|`, where `v` points from
    /// the surface to the eye (orthographic: against the view direction).
    #[default]
    Headlight,
    /// No shading: every surface in its own colour.
    Unlit,
}

/// What [`render`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Image width, pixels.
    pub width: u32,
    /// Image height, pixels.
    pub height: u32,
    /// Background colour, straight RGBA.
    pub background: [u8; 4],
    /// The colour of a surface with none of its own.
    pub colour: [u8; 3],
    /// How surfaces are lit.
    pub lighting: Lighting,
}

impl Default for RenderOptions {
    /// 1024 × 768, opaque white background, light grey surface, the headlight.
    fn default() -> Self {
        RenderOptions { width: 1024, height: 768, background: [255, 255, 255, 255], colour: [190, 192, 200], lighting: Lighting::Headlight }
    }
}

/// A rendered image: `rgba` holds `width * height` straight-alpha pixels, row by row from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width, pixels.
    pub width: u32,
    /// Height, pixels.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// Why [`render`], [`Camera::fit`] or [`crate::assemble`] refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// The image is empty or larger than [`MAX_RENDER_PIXELS`].
    #[error("a {width}x{height} image is empty or larger than the render ceiling")]
    Size {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// The camera cannot form a view.
    #[error("invalid camera: {0}")]
    Camera(&'static str),
    /// The model could not be read.
    #[error("unreadable model: {0}")]
    Model(#[from] craft_prc::PrcError),
    /// The placed model has more than [`crate::MAX_ASSEMBLED_TRIANGLES`] triangles.
    #[error("the placed model has more triangles than the assembly ceiling")]
    TooManyTriangles,
    /// The placed model has more than [`crate::MAX_ASSEMBLED_VERTICES`] positions, normals and
    /// texture coordinates.
    #[error("the placed model has more vertices than the assembly ceiling")]
    TooManyVertices,
    /// Drawing would test more than [`MAX_RENDER_WORK`] pixel centres.
    #[error("the render would test more pixels than the work ceiling")]
    TooMuchWork,
}

/// Draw `meshes` as seen by `camera`.
///
/// Each triangle is filled in its own colour ([`WorldMesh::triangle_colours`]), else its mesh's
/// ([`WorldMesh::colour`]), else [`RenderOptions::colour`], and lit per [`RenderOptions::lighting`];
/// nearer surfaces hide farther ones. Opaque triangles are drawn first; translucent ones are then
/// blended over them, hidden by nearer opaque surfaces but not by each other, in mesh order; fully
/// transparent ones are not drawn. Triangles with a non-finite or out-of-range vertex are skipped.
/// A perspective camera clips at a near plane 1/10,000 of the way from the eye to the target, so
/// what lies behind the eye, or in front of it but nearer than that, is not drawn; an orthographic
/// camera does not clip.
///
/// # Errors
/// [`RenderError::Size`] for an empty or over-ceiling image; [`RenderError::Camera`] for a camera
/// that cannot form a view (eye on the target, `up` parallel to the view, a field of view outside
/// 0–180° or too small for a finite focal length, a non-positive orthographic height);
/// [`RenderError::TooMuchWork`] past [`MAX_RENDER_WORK`].
pub fn render(meshes: &[WorldMesh], camera: &Camera, options: &RenderOptions) -> Result<Image, RenderError> {
    render_in(meshes, camera, options, &Look::default(), None).map(|f| f.image)
}

/// How [`render_look`] draws the meshes beyond the camera: some hidden, some highlighted, the
/// model cut by a section plane. Each list is indexed like the meshes; a missing entry is false.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    /// Meshes not drawn.
    pub hidden: Vec<bool>,
    /// Meshes tinted towards [`HIGHLIGHT`], as a viewer marks the picked part.
    pub highlighted: Vec<bool>,
    /// What lies past this plane is cut away.
    pub section: Option<Section>,
    /// Per mesh, the edges drawn over it in [`OUTLINE`] where nothing hides them (as
    /// [`crate::outline_edges`] finds them): its faces' borders, as CAD programs draw them.
    pub outlines: Option<std::sync::Arc<[Vec<[u32; 2]>]>>,
}

impl Look {
    fn hidden(&self, mesh: usize) -> bool {
        self.hidden.get(mesh).copied().unwrap_or(false)
    }

    fn highlighted(&self, mesh: usize) -> bool {
        self.highlighted.get(mesh).copied().unwrap_or(false)
    }
}

/// A section plane: the points `p` with `normal · p > offset` (world coordinates) are cut away.
/// Where it opens a part, the inside faces seen through the cut are drawn flat and darker, so a
/// closed part reads as solid. A plane with a non-finite value cuts nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// The direction from the kept side to the cut side; any length.
    pub normal: [f64; 3],
    /// Where the plane lies along `normal`, in its units.
    pub offset: f64,
}

/// The colour of [`Look::outlines`].
pub const OUTLINE: [u8; 4] = [24, 24, 28, 255];

/// The colour [`Look::highlighted`] meshes are tinted towards.
pub const HIGHLIGHT: [u8; 3] = [40, 130, 255];

/// How much of [`HIGHLIGHT`] a highlighted mesh's colour takes.
const HIGHLIGHT_SHARE: f64 = 0.65;

/// The shade of an inside face seen through a section.
const INSIDE_SHADE: f64 = 0.55;

/// What [`render_look`] drew: the image, which mesh each pixel shows, and how far away.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The picture.
    pub image: Image,
    /// Per pixel, row by row from the top, the index of the mesh drawn there last (a
    /// translucent one over an opaque one counts); [`NO_ID`] where only the background shows.
    ids: Vec<u32>,
    /// Per pixel, the depth key of the nearest opaque surface ([`Target`]'s), negative infinity
    /// where there is none.
    depth: Vec<f64>,
}

impl Frame {
    /// The mesh drawn at pixel (`x`, `y`), as a viewer picks the part under the pointer.
    #[must_use]
    pub fn mesh_at(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.image.width || y >= self.image.height {
            return None;
        }
        let i = pixel_index(self.image.width, x, y)?;
        self.ids.get(i).copied().filter(|&id| id != NO_ID).and_then(|id| usize::try_from(id).ok())
    }

    /// The point on the nearest opaque surface seen at the centre of pixel (`x`, `y`), in world
    /// coordinates, as `camera` (the one the frame was drawn with) sees it: what a viewer turns
    /// the model about when a drag starts there. `None` where no opaque surface shows.
    #[must_use]
    pub fn point_at(&self, x: u32, y: u32, camera: &Camera) -> Option<[f64; 3]> {
        if x >= self.image.width || y >= self.image.height {
            return None;
        }
        let key = *self.depth.get(pixel_index(self.image.width, x, y)?)?;
        if !key.is_finite() {
            return None;
        }
        let (w, h) = (f64::from(self.image.width), f64::from(self.image.height));
        let [right, up, forward] = camera.basis().ok()?;
        // The pixel centre in normalised device coordinates, y up.
        let (nx, ny) = ((f64::from(x) + 0.5) / w * 2.0 - 1.0, 1.0 - (f64::from(y) + 0.5) / h * 2.0);
        let aspect = w / h;
        // The inverse of `View::project`.
        let [vx, vy, vz] = match camera.projection {
            Projection::Perspective { fov_y } => {
                let focal = 1.0 / (fov_y.to_radians() / 2.0).tan();
                let z = 1.0 / key;
                [nx * z * aspect / focal, ny * z / focal, z]
            }
            Projection::Orthographic { height } => {
                let half = height / 2.0;
                [nx * half * aspect, ny * half, -key]
            }
        };
        let p = [0, 1, 2].map(|k| at(camera.eye, k) + at(right, k) * vx + at(up, k) * vy + at(forward, k) * vz);
        p.iter().all(|c| c.is_finite()).then_some(p)
    }
}

/// [`render`] as `look` says, also saying which mesh each pixel shows.
///
/// # Errors
/// As [`render`].
pub fn render_look(meshes: &[WorldMesh], camera: &Camera, options: &RenderOptions, look: &Look) -> Result<Frame, RenderError> {
    render_in(meshes, camera, options, look, None)
}

/// One worker's drawing: colours, depth keys and ids.
struct Layer {
    rgba: Vec<u8>,
    depth: Vec<f64>,
    ids: Vec<u32>,
}

/// [`render_look`] with `workers_override` in place of the machine's choice of workers (tests
/// compare the two).
fn render_in(
    meshes: &[WorldMesh],
    camera: &Camera,
    options: &RenderOptions,
    look: &Look,
    workers_override: Option<usize>,
) -> Result<Frame, RenderError> {
    let workers = |triangles: usize, pixels: usize| workers_override.unwrap_or_else(|| workers(triangles, pixels));
    let (width, height) = (options.width, options.height);
    let size = RenderError::Size { width, height };
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_RENDER_PIXELS {
        return Err(size);
    }
    let pixels = usize::try_from(pixels).map_err(|_| size)?;
    let view = View::new(camera, f64::from(width) / f64::from(height))?;
    let perspective = matches!(view, View::Perspective { .. });
    let section = look.section.filter(|s| s.normal.iter().all(|v| v.is_finite()) && s.offset.is_finite());
    let scene = Scene { eye: camera.eye, basis: camera.basis()?, view, lighting: options.lighting, section };
    let [r, g, b] = options.colour;
    let colour_of = |k: usize, m: &WorldMesh, t: usize| {
        let c = m.colour_of(t).unwrap_or([r, g, b, 255]);
        if look.highlighted(k) { tint(c) } else { c }
    };
    let id_of = |k: usize| u32::try_from(k).unwrap_or(NO_ID);
    let pool = AtomicU64::new(MAX_RENDER_WORK);
    let layer = || Layer { rgba: options.background.repeat(pixels), depth: vec![f64::NEG_INFINITY; pixels], ids: vec![NO_ID; pixels] };
    // Opaque triangles, in order, shared out over workers that each draw a run of them into
    // their own layer; the layers are then merged in the same order.
    let total: usize = meshes.iter().enumerate().filter(|&(k, _)| !look.hidden(k)).map(|(_, m)| m.mesh.triangles.len()).sum();
    let runs = split(total, workers(total, pixels));
    let draw_run = |run: Range<usize>, l: &mut Layer| -> Result<(), RenderError> {
        let mut target = Target {
            perspective,
            width,
            height,
            rgba: &mut l.rgba,
            depth: &mut l.depth,
            ids: &mut l.ids,
            work: 0,
            pool: Some(&pool),
            rows: 0..height,
        };
        for (k, m, t) in triangles(meshes, look, run) {
            let colour = colour_of(k, m, t);
            if colour[3] == 255 {
                scene.draw(&mut target, &m.mesh, t, colour, id_of(k))?;
            }
        }
        Ok(())
    };
    let mut layers: Vec<Layer> = runs.iter().map(|_| layer()).collect();
    in_parallel(&runs, &mut layers, &draw_run)?;
    let mut layers = layers.into_iter();
    let mut out = layers.next().unwrap_or_else(layer);
    for more in layers {
        merge(&mut out, &more);
    }
    // Then the translucent ones, over everything opaque, in order: blending depends on order,
    // so these are shared out by bands of rows, each band drawing every one of them.
    let translucent = |k: usize, m: &WorldMesh, t: usize| {
        let a = colour_of(k, m, t)[3];
        a != 255 && a != 0
    };
    let shown = || meshes.iter().enumerate().filter(|&(k, _)| !look.hidden(k));
    let count = shown().map(|(k, m)| (0..m.mesh.triangles.len()).filter(|&t| translucent(k, m, t)).count()).sum::<usize>();
    if count > 0 {
        // Both sides are at most the image's pixel count, which fits a usize.
        let (row_len, rows) = (usize::try_from(width).unwrap_or(0), usize::try_from(height).unwrap_or(0));
        let bands = split(rows, workers(count, 0).min(rows));
        let (mut rgba_rest, mut depth_rest, mut ids_rest): (&mut [u8], &mut [f64], &mut [u32]) = (&mut out.rgba, &mut out.depth, &mut out.ids);
        let mut pieces = Vec::with_capacity(bands.len());
        for band in &bands {
            let n = band.len() * row_len;
            let (r, rest) = std::mem::take(&mut rgba_rest).split_at_mut(n * 4);
            let (d, drest) = std::mem::take(&mut depth_rest).split_at_mut(n);
            let (i, irest) = std::mem::take(&mut ids_rest).split_at_mut(n);
            (rgba_rest, depth_rest, ids_rest) = (rest, drest, irest);
            pieces.push((band.clone(), r, d, i));
        }
        let draw_band = |(rows, rgba, depth, ids): &mut (Range<usize>, &mut [u8], &mut [f64], &mut [u32])| -> Result<(), RenderError> {
            let rows = u32::try_from(rows.start).unwrap_or(0)..u32::try_from(rows.end).unwrap_or(0);
            let mut target = Target { perspective, width, height, rgba, depth, ids, work: 0, pool: Some(&pool), rows };
            for (k, m) in shown() {
                for t in (0..m.mesh.triangles.len()).filter(|&t| translucent(k, m, t)) {
                    scene.draw(&mut target, &m.mesh, t, colour_of(k, m, t), id_of(k))?;
                }
            }
            Ok(())
        };
        each_in_parallel(&mut pieces, &draw_band)?;
    }
    // Then the edges, over everything, where nothing nearer hides them: each worker finds the
    // pixels of a run of them, and those are inked once all have looked.
    if let Some(outlines) = &look.outlines {
        let edges: usize = shown().map(|(k, _)| outlines.get(k).map_or(0, Vec::len)).sum();
        let runs = split(edges, workers(edges.saturating_mul(8), 0));
        let depth = &out.depth;
        let find = |run: Range<usize>, found: &mut Vec<usize>| {
            let mut first = 0;
            for (k, m) in shown() {
                let lines = outlines.get(k).map_or(&[][..], Vec::as_slice);
                let (lo, hi) = (run.start.saturating_sub(first).min(lines.len()), run.end.saturating_sub(first).min(lines.len()));
                first += lines.len();
                for &[a, b] in lines.get(lo..hi).unwrap_or(&[]) {
                    scene.line(&m.mesh, [a, b], (width, height), depth, found);
                }
            }
        };
        let mut found: Vec<(Range<usize>, Vec<usize>)> = runs.into_iter().map(|r| (r, Vec::new())).collect();
        each_in_parallel(&mut found, &|(run, pixels): &mut (Range<usize>, Vec<usize>)| {
            find(run.clone(), pixels);
            Ok(())
        })?;
        for i in found.iter().flat_map(|(_, p)| p) {
            if let Some(px) = i.checked_mul(4).and_then(|s| out.rgba.get_mut(s..s.checked_add(4)?)) {
                px.copy_from_slice(&OUTLINE);
            }
        }
    }
    Ok(Frame { image: Image { width, height, rgba: out.rgba }, ids: out.ids, depth: out.depth })
}

/// `colour` tinted towards [`HIGHLIGHT`]; alpha kept.
fn tint([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let mix = |c: u8, h: u8| channel(f64::from(c) * (1.0 - HIGHLIGHT_SHARE) + f64::from(h) * HIGHLIGHT_SHARE);
    let [hr, hg, hb] = HIGHLIGHT;
    [mix(r, hr), mix(g, hg), mix(b, hb), a]
}

/// The most layers [`render`] draws at once, and the most memory they may take.
const MAX_WORKERS: usize = 8;
const MAX_LAYER_BYTES: usize = 256 << 20;

/// How many workers draw `triangles` into images of `pixels`: one per core, up to
/// [`MAX_WORKERS`], each with at least 50,000 triangles and their layers within [`MAX_LAYER_BYTES`].
fn workers(triangles: usize, pixels: usize) -> usize {
    #[cfg(target_arch = "wasm32")]
    let cores = 1;
    #[cfg(not(target_arch = "wasm32"))]
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    // Bands of one image (`pixels` 0) take no extra memory.
    let by_memory = if pixels == 0 { usize::MAX } else { MAX_LAYER_BYTES / pixels.saturating_mul(12) };
    cores.min(MAX_WORKERS).min(triangles / 50_000).min(by_memory).max(1)
}

/// `0..total` cut into `parts` consecutive runs of near-equal length.
fn split(total: usize, parts: usize) -> Vec<Range<usize>> {
    let parts = parts.max(1);
    (0..parts).map(|k| total * k / parts..total * (k + 1) / parts).collect()
}

/// The triangles numbered `run` when the shown meshes' triangles are counted one after another,
/// each with its mesh's index.
fn triangles<'m>(meshes: &'m [WorldMesh], look: &'m Look, run: Range<usize>) -> impl Iterator<Item = (usize, &'m WorldMesh, usize)> + 'm {
    let mut first = 0;
    meshes.iter().enumerate().filter(move |&(k, _)| !look.hidden(k)).flat_map(move |(k, m)| {
        let n = m.mesh.triangles.len();
        let (lo, hi) = (run.start.saturating_sub(first).min(n), run.end.saturating_sub(first).min(n));
        first += n;
        (lo..hi).map(move |t| (k, m, t))
    })
}

/// Draw each run into its layer, on its own thread when there is more than one.
fn in_parallel<F>(runs: &[Range<usize>], layers: &mut [Layer], draw: &F) -> Result<(), RenderError>
where
    F: Fn(Range<usize>, &mut Layer) -> Result<(), RenderError> + Sync,
{
    let mut jobs: Vec<_> = runs.iter().cloned().zip(layers.iter_mut()).collect();
    each_in_parallel(&mut jobs, &|(run, layer): &mut (Range<usize>, &mut Layer)| draw(run.clone(), layer))
}

/// Run `work` on every job, each on its own thread when there is more than one (one thread on
/// wasm32); the first error, if any.
fn each_in_parallel<J: Send, F>(jobs: &mut [J], work: &F) -> Result<(), RenderError>
where
    F: Fn(&mut J) -> Result<(), RenderError> + Sync,
{
    #[cfg(not(target_arch = "wasm32"))]
    if jobs.len() > 1 {
        return std::thread::scope(|s| {
            let handles: Vec<_> = jobs.iter_mut().map(|job| s.spawn(move || work(job))).collect();
            // A worker that panicked has drawn nothing usable: the frame is refused as too much work.
            handles.into_iter().map(|h| h.join().unwrap_or(Err(RenderError::TooMuchWork))).collect::<Result<Vec<()>, _>>().map(|_| ())
        });
    }
    jobs.iter_mut().try_for_each(work)
}

/// Merge a later layer into `out`: its pixel wins where it is strictly nearer, as a later opaque
/// triangle drawn into the same layer would.
fn merge(out: &mut Layer, more: &Layer) {
    let mine = out.depth.iter_mut().zip(out.rgba.as_chunks_mut::<4>().0).zip(out.ids.iter_mut());
    let theirs = more.depth.iter().zip(more.rgba.as_chunks::<4>().0).zip(&more.ids);
    for (((d, px), id), ((&md, mpx), &mid)) in mine.zip(theirs) {
        if md > *d {
            *d = md;
            *px = *mpx;
            *id = mid;
        }
    }
}

/// The camera a frame is drawn from, with its basis and projection.
struct Scene {
    eye: [f64; 3],
    /// Right, up and forward.
    basis: [[f64; 3]; 3],
    view: View,
    lighting: Lighting,
    section: Option<Section>,
}

impl Scene {
    /// From world point `p` towards the eye (orthographic: against the view direction).
    fn towards_eye(&self, p: [f64; 3]) -> [f64; 3] {
        match self.view {
            View::Perspective { .. } => sub(self.eye, p),
            View::Orthographic { .. } => scale(at3(&self.basis, 2), -1.0),
        }
    }

    /// The shade at world point `p` with normal `n`.
    fn shade(&self, p: [f64; 3], n: [f64; 3]) -> [f64; 3] {
        if self.lighting == Lighting::Unlit {
            return [1.0; 3];
        }
        let towards = match self.view {
            View::Perspective { .. } => normalize(sub(self.eye, p)),
            View::Orthographic { .. } => Some(scale(at3(&self.basis, 2), -1.0)),
        };
        [0.3 + 0.7 * towards.map_or(1.0, |v| dot(n, v).abs().min(1.0)); 3]
    }

    /// The pixels of `mesh`'s edge between corners `a` and `b` that nothing nearer in `depth`
    /// hides, in a picture `size` big; pushed onto `out`.
    fn line(&self, mesh: &TriangleMesh, [a, b]: [u32; 2], size: (u32, u32), depth: &[f64], out: &mut Vec<usize>) {
        let corner = |i: u32| usize::try_from(i).ok().and_then(|i| mesh.positions.get(i).copied()).filter(|p| p.iter().all(|c| c.is_finite()));
        let (Some(pa), Some(pb)) = (corner(a), corner(b)) else { return };
        let [right, up, forward] = self.basis;
        let past = |p: [f64; 3]| self.section.map_or(0.0, |s| dot(s.normal, p) - s.offset);
        let view = |p: [f64; 3]| -> ViewCorner {
            let d = sub(p, self.eye);
            [dot(d, right), dot(d, up), dot(d, forward), 0.0, 0.0, 0.0, past(p)]
        };
        let (mut va, mut vb) = (view(pa), view(pb));
        // A perspective line is cut where it passes the near plane.
        if let View::Perspective { near, .. } = self.view {
            let z = |v: &ViewCorner| v.get(2).copied().unwrap_or(0.0);
            match (z(&va) >= near, z(&vb) >= near) {
                (false, false) => return,
                (true, true) => {}
                (a_in, _) => {
                    let (inside, outside) = if a_in { (va, vb) } else { (vb, va) };
                    let t = (near - z(&inside)) / (z(&outside) - z(&inside));
                    let mut cut = inside;
                    for (c, o) in cut.iter_mut().zip(outside) {
                        *c += t * (o - *c);
                    }
                    if a_in { vb = cut } else { va = cut }
                }
            }
        }
        let perspective = matches!(self.view, View::Perspective { .. });
        let (sa, sb) = (self.view.project(va, size.0, size.1), self.view.project(vb, size.0, size.1));
        crate::raster::line_pixels(sa, sb, size, depth, perspective, self.section.is_some(), out);
    }

    /// Draw triangle `t` of `mesh` into `target` in straight-RGBA `colour`; refused once the
    /// target's work allowance is spent.
    fn draw(&self, target: &mut Target, mesh: &TriangleMesh, t: usize, colour: [u8; 4], id: u32) -> Result<(), RenderError> {
        let Some(world) = mesh.triangles.get(t).and_then(|tri| corners(mesh, tri)) else {
            return Ok(());
        };
        let Some(face) = unit_cross(span(at3(&world, 0), at3(&world, 1)), span(at3(&world, 0), at3(&world, 2))) else {
            return Ok(());
        };
        let stored = stored_normals(mesh, t);
        let normal = |k: usize| stored.and_then(|n| n.get(k).copied().flatten()).unwrap_or(face);
        // Without stored normals the triangle is flat: one shade, lit along the view ray to its first corner.
        let flat = stored.is_none().then(|| self.shade(at3(&world, 0), normal(0)));
        // With a section on, a face turned away from the eye is the inside of a cut part: flat
        // and darker, so the cut reads as solid.
        let inside = self.section.is_some() && dot(face, self.towards_eye(at3(&world, 0))) < 0.0;
        let flat = if inside { Some([INSIDE_SHADE; 3]) } else { flat };
        let past = |p: [f64; 3]| self.section.map_or(0.0, |s| dot(s.normal, p) - s.offset);
        let [right, up, forward] = self.basis;
        let view_corners = [0, 1, 2].map(|k| -> ViewCorner {
            let p = at3(&world, k);
            let [r, g, b] = flat.unwrap_or_else(|| self.shade(p, normal(k)));
            let d = sub(p, self.eye);
            [dot(d, right), dot(d, up), dot(d, forward), r, g, b, past(p)]
        });
        let clipped = self.view.clip(view_corners);
        let (width, height) = (target.width, target.height);
        let screen = clipped.as_slice().iter().map(|&v| self.view.project(v, width, height));
        let mut screen = screen.fuse();
        if let (Some(a), Some(mut b)) = (screen.next(), screen.next()) {
            for c in screen {
                target.fill([a, b, c], colour, id, self.section.is_some())?;
                b = c;
            }
        }
        Ok(())
    }
}

/// Triangle `t`'s stored corner normals, unit length; a corner whose normal is zero or not finite
/// is `None`, and so is the whole when the mesh stores none.
fn stored_normals(mesh: &TriangleMesh, t: usize) -> Option<[Option<[f64; 3]>; 3]> {
    let slots = mesh.triangle_normals.get(t)?;
    Some(slots.map(|i| mesh.normals.get(usize::try_from(i).ok()?).copied().and_then(normalize)))
}

/// The three corners of a triangle, or `None` when one is missing or not finite.
fn corners(mesh: &TriangleMesh, tri: &[u32; 3]) -> Option<[[f64; 3]; 3]> {
    let mut out = [[0.0; 3]; 3];
    for (slot, &i) in out.iter_mut().zip(tri) {
        let p = *mesh.positions.get(usize::try_from(i).ok()?)?;
        if !p.iter().all(|c| c.is_finite()) {
            return None;
        }
        *slot = p;
    }
    Some(out)
}

/// Corner `i` of a triangle, the origin when out of range.
fn at3(v: &[[f64; 3]; 3], i: usize) -> [f64; 3] {
    v.get(i).copied().unwrap_or([0.0; 3])
}

#[cfg(test)]
mod tests;
