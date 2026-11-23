//! Subtle, proximity-revealed 3D axis limit gizmos and interactive drag handles.
//!
//! Projected using the exact same stereographic-to-perspective blended projection as the
//! 3D mount renderer (`PlanetariumCamera::projection_for_fov`), so arcs and handles sit
//! directly on the NEQ6-Pro's physical RA, Altitude, and Azimuth axes with zero drift.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use celestial::projection::CameraProjection;
use glam::{DVec3, Vec3};
use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, point, px, rgb};
use mount_renderer::{COMPASS_RADIUS, MountSceneState, OrbitCamera, PIVOT_Y};
use protocol::MountLimits;

const RA_RING_OFFSET: f32 = 0.14;
const RA_RING_RADIUS: f32 = 0.44;
const ALT_ARC_RADIUS: f32 = 0.78;

/// Distance in logical pixels at which an axis gizmo begins fading in.
const PROXIMITY_FADE_OUTER_PX: f32 = 110.0;
/// Distance in logical pixels at which an axis gizmo reaches full subtle visibility.
const PROXIMITY_FADE_INNER_PX: f32 = 34.0;
/// Hit radius in logical pixels for grabbing a drag handle.
const HANDLE_HIT_RADIUS_PX: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoHandle {
    MinAltitude,
    MaxAltitude,
    MeridianLimit,
    AzAvoidStart,
    AzAvoidEnd,
}

impl GizmoHandle {
    pub fn label_and_value(self, limits: &MountLimits) -> String {
        match self {
            Self::MinAltitude => format!("Min Alt {:.0}°", limits.min_altitude_deg),
            Self::MaxAltitude => format!("Max Alt {:.0}°", limits.max_altitude_deg),
            Self::MeridianLimit => format!("Meridian +{:.0}°", limits.meridian_limit_deg),
            Self::AzAvoidStart => format!("Az Start {:.0}°", limits.az_avoid_start_deg),
            Self::AzAvoidEnd => format!("Az End {:.0}°", limits.az_avoid_end_deg),
        }
    }

    pub fn accent_color(self) -> Hsla {
        match self {
            Self::MinAltitude => rgb(0xf59447).into(),
            Self::MaxAltitude | Self::AzAvoidStart | Self::AzAvoidEnd => rgb(0xf25c66).into(),
            Self::MeridianLimit => rgb(0x59b8fa).into(),
        }
    }

    fn axis_group(self) -> AxisGroup {
        match self {
            Self::MinAltitude | Self::MaxAltitude => AxisGroup::Altitude,
            Self::MeridianLimit => AxisGroup::Meridian,
            Self::AzAvoidStart | Self::AzAvoidEnd => AxisGroup::Azimuth,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AxisGroup {
    Altitude,
    Meridian,
    Azimuth,
}

#[derive(Clone, Copy, Debug)]
struct AxisFrames {
    /// RA polar ring center & orthonormal equatorial basis `(u_meridian, u_west, u_pole)`.
    ra_center: Vec3,
    u_meridian: Vec3,
    u_west: Vec3,
    u_pole: Vec3,
    /// Vertical altitude arc center (`pivot`) & orthonormal plane `(u_az_horiz, Vec3::Y, u_az_normal)`.
    alt_center: Vec3,
    u_az_horiz: Vec3,
    u_az_normal: Vec3,
}

impl AxisFrames {
    fn from_scene(scene: &MountSceneState) -> Self {
        let eq = scene.equatorial_frame();
        let u_west = -eq.u_east;
        let pivot = Vec3::new(0.0, PIVOT_Y, 0.0);

        // Align the vertical altitude gizmo arc with the OTA's horizontal azimuth so the user
        // directly sees the telescope elevation relative to Min Alt and Max Alt.
        let ota = scene.ota_direction();
        let horiz = Vec3::new(ota.x, 0.0, ota.z);
        let u_az_horiz = if horiz.length_squared() > 1e-4 {
            horiz.normalize()
        } else {
            let az = (scene.az_deg as f32).to_radians();
            Vec3::new(az.sin(), 0.0, -az.cos())
        };
        let u_az_normal = u_az_horiz.cross(Vec3::Y).normalize_or_zero();

        Self {
            ra_center: pivot + eq.u_elevated_pole * RA_RING_OFFSET,
            u_meridian: eq.u_meridian,
            u_west,
            u_pole: eq.u_pole,
            alt_center: pivot,
            u_az_horiz,
            u_az_normal,
        }
    }

    fn ra_point(&self, ha_rad: f32) -> Vec3 {
        let (s, c) = ha_rad.sin_cos();
        self.ra_center + (self.u_meridian * c + self.u_west * s) * RA_RING_RADIUS
    }

    fn alt_point(&self, alt_rad: f32) -> Vec3 {
        let (s, c) = alt_rad.sin_cos();
        self.alt_center + (self.u_az_horiz * c + Vec3::Y * s) * ALT_ARC_RADIUS
    }

    fn az_point(az_rad: f32) -> Vec3 {
        let (s, c) = az_rad.sin_cos();
        Vec3::new(s * COMPASS_RADIUS, 0.012, -c * COMPASS_RADIUS)
    }
}

/// Projects a world-space point (`+X` East, `+Y` Up, `-Z` North) into window-space logical pixels
/// inside `bounds` using the exact same stereographic/blended projection as `mount.wgsl`.
pub fn project_world_point(
    world_pos: Vec3,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
) -> Option<Point<Pixels>> {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if bw <= 2.0 || bh <= 2.0 {
        return None;
    }

    let aspect = (bw / bh).max(0.1);
    let p_view = camera.view_matrix().transform_point3(world_pos);
    let v_cam = DVec3::new(
        f64::from(p_view.x),
        f64::from(p_view.y),
        f64::from(-p_view.z),
    );
    let proj = CameraProjection::for_fov(f64::from(camera.fov_y_deg).to_radians(), aspect);
    let (nx, ny) = proj.project_cam_dir(v_cam)?;
    if !nx.is_finite() || !ny.is_finite() || nx.abs() > 2.2 || ny.abs() > 2.2 {
        return None;
    }

    let sx = f32::from(bounds.origin.x) + ((nx as f32) * 0.5 + 0.5) * bw;
    let sy = f32::from(bounds.origin.y) + (0.5 - (ny as f32) * 0.5) * bh;
    Some(point(px(sx), px(sy)))
}

/// Unprojects a window-space logical pixel position into a world-space ray `(origin, dir)`.
pub fn ray_from_screen(
    pos: Point<Pixels>,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
) -> Option<(Vec3, Vec3)> {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if bw <= 2.0 || bh <= 2.0 {
        return None;
    }

    let local_x = f32::from(pos.x) - f32::from(bounds.origin.x);
    let local_y = f32::from(pos.y) - f32::from(bounds.origin.y);
    let nx = f64::from((local_x / bw) * 2.0 - 1.0);
    let ny = f64::from(1.0 - (local_y / bh) * 2.0);
    let aspect = (bw / bh).max(0.1);

    let proj = CameraProjection::for_fov(f64::from(camera.fov_y_deg).to_radians(), aspect);
    let v_cam = proj.unproject_ndc(nx, ny);
    let p_view = Vec3::new(v_cam.x as f32, v_cam.y as f32, -(v_cam.z as f32));
    let ray_dir = camera
        .view_matrix()
        .inverse()
        .transform_vector3(p_view)
        .normalize_or_zero();
    if ray_dir.length_squared() < 1e-6 {
        return None;
    }
    Some((camera.eye_position(), ray_dir))
}

fn intersect_plane(
    origin: Vec3,
    dir: Vec3,
    plane_origin: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let denom = dir.dot(plane_normal);
    if denom.abs() < 1e-4 {
        return None;
    }
    let t = (plane_origin - origin).dot(plane_normal) / denom;
    if t <= 0.0 || !t.is_finite() {
        return None;
    }
    Some(origin + dir * t)
}

fn dist_px(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let dx = f32::from(a.x) - f32::from(b.x);
    let dy = f32::from(a.y) - f32::from(b.y);
    dx.hypot(dy)
}

fn min_polyline_dist(cursor: Point<Pixels>, pts: &[Point<Pixels>]) -> f32 {
    if pts.is_empty() {
        return f32::INFINITY;
    }
    if pts.len() == 1 {
        return dist_px(cursor, pts[0]);
    }
    let cx = f32::from(cursor.x);
    let cy = f32::from(cursor.y);
    let mut best = f32::INFINITY;
    for pair in pts.windows(2) {
        let ax = f32::from(pair[0].x);
        let ay = f32::from(pair[0].y);
        let bx = f32::from(pair[1].x);
        let by = f32::from(pair[1].y);
        let abx = bx - ax;
        let aby = by - ay;
        let len2 = abx * abx + aby * aby;
        let t = if len2 > 1e-5 {
            (((cx - ax) * abx + (cy - ay) * aby) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let px_c = ax + abx * t;
        let py_c = ay + aby * t;
        let d = (cx - px_c).hypot(cy - py_c);
        if d < best {
            best = d;
        }
    }
    best
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let denom = (edge1 - edge0).max(1e-6);
    let t = ((x - edge0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn proximity_alpha(min_dist: f32) -> f32 {
    1.0 - smoothstep(PROXIMITY_FADE_INNER_PX, PROXIMITY_FADE_OUTER_PX, min_dist)
}

/// Samples an arc into one or more continuous screen-space polylines, breaking whenever a
/// point projects behind the camera so we never draw spurious straight lines across the viewport.
fn sample_arc_segments(
    steps: usize,
    start: f32,
    end: f32,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
    mut point_fn: impl FnMut(f32) -> Vec3,
) -> Vec<Vec<Point<Pixels>>> {
    let mut segments: Vec<Vec<Point<Pixels>>> = Vec::new();
    let mut current: Vec<Point<Pixels>> = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = start + (end - start) * t;
        if let Some(p) = project_world_point(point_fn(angle), camera, bounds) {
            current.push(p);
        } else if !current.is_empty() {
            segments.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

fn min_segments_dist(cursor: Point<Pixels>, segments: &[Vec<Point<Pixels>>]) -> f32 {
    segments
        .iter()
        .map(|seg| min_polyline_dist(cursor, seg))
        .fold(f32::INFINITY, f32::min)
}

fn paint_arc_segments(
    segments: &[Vec<Point<Pixels>>],
    width: f32,
    color: Hsla,
    window: &mut Window,
) {
    for seg in segments {
        paint_polyline(seg, width, color, window);
    }
}

fn handle_world_position(handle: GizmoHandle, frames: &AxisFrames, limits: &MountLimits) -> Vec3 {
    match handle {
        GizmoHandle::MinAltitude => frames.alt_point((limits.min_altitude_deg as f32).to_radians()),
        GizmoHandle::MaxAltitude => frames.alt_point((limits.max_altitude_deg as f32).to_radians()),
        GizmoHandle::MeridianLimit => {
            frames.ra_point((limits.meridian_limit_deg as f32).to_radians())
        }
        GizmoHandle::AzAvoidStart => {
            AxisFrames::az_point((limits.az_avoid_start_deg as f32).to_radians())
        }
        GizmoHandle::AzAvoidEnd => {
            AxisFrames::az_point((limits.az_avoid_end_deg as f32).to_radians())
        }
    }
}

/// Returns the projected screen position of a specific handle (for tooltip/label anchoring).
pub fn handle_screen_position(
    handle: GizmoHandle,
    scene: &MountSceneState,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
) -> Option<Point<Pixels>> {
    let frames = AxisFrames::from_scene(scene);
    project_world_point(handle_world_position(handle, &frames, &scene.limits), camera, bounds)
}

/// Hit-tests all enabled limit gizmo handles and returns the closest one within grab radius.
pub fn hit_test_handle(
    cursor: Point<Pixels>,
    scene: &MountSceneState,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
) -> Option<GizmoHandle> {
    let frames = AxisFrames::from_scene(scene);
    let mut best: Option<(GizmoHandle, f32)> = None;
    let mut check = |handle: GizmoHandle| {
        let world = handle_world_position(handle, &frames, &scene.limits);
        if let Some(pos) = project_world_point(world, camera, bounds) {
            let d = dist_px(cursor, pos);
            if d <= HANDLE_HIT_RADIUS_PX && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((handle, d));
            }
        }
    };

    if scene.widgets.altitude_limits {
        check(GizmoHandle::MinAltitude);
        check(GizmoHandle::MaxAltitude);
    }
    if scene.widgets.meridian_flip {
        check(GizmoHandle::MeridianLimit);
    }
    if scene.widgets.azimuth_sector && scene.limits.az_avoid_enabled {
        check(GizmoHandle::AzAvoidStart);
        check(GizmoHandle::AzAvoidEnd);
    }
    best.map(|(h, _)| h)
}

/// Computes updated `MountLimits` while interactively dragging `handle` to `cursor`.
pub fn drag_handle_to_limits(
    handle: GizmoHandle,
    cursor: Point<Pixels>,
    scene: &MountSceneState,
    camera: OrbitCamera,
    bounds: Bounds<Pixels>,
) -> Option<MountLimits> {
    let (ray_origin, ray_dir) = ray_from_screen(cursor, camera, bounds)?;
    let frames = AxisFrames::from_scene(scene);
    let mut limits = scene.limits;

    match handle {
        GizmoHandle::MinAltitude | GizmoHandle::MaxAltitude => {
            let hit = intersect_plane(ray_origin, ray_dir, frames.alt_center, frames.u_az_normal)?;
            let local = hit - frames.alt_center;
            let x = local.dot(frames.u_az_horiz);
            let y = local.dot(Vec3::Y);
            let alt_deg = f64::from(y.atan2(x.max(0.0)).to_degrees()).round();
            if handle == GizmoHandle::MinAltitude {
                limits.min_altitude_deg =
                    alt_deg.clamp(0.0, (limits.max_altitude_deg - 5.0).clamp(0.0, 80.0));
            } else {
                limits.max_altitude_deg =
                    alt_deg.clamp((limits.min_altitude_deg + 5.0).min(90.0), 90.0);
            }
        }
        GizmoHandle::MeridianLimit => {
            let hit = intersect_plane(ray_origin, ray_dir, frames.ra_center, frames.u_pole)?;
            let local = hit - frames.ra_center;
            let cos_h = local.dot(frames.u_meridian);
            let sin_h = local.dot(frames.u_west);
            let ha_deg = f64::from(sin_h.atan2(cos_h).to_degrees()).round();
            limits.meridian_limit_deg = ha_deg.clamp(0.0, 45.0);
        }
        GizmoHandle::AzAvoidStart | GizmoHandle::AzAvoidEnd => {
            let hit = intersect_plane(ray_origin, ray_dir, Vec3::new(0.0, 0.012, 0.0), Vec3::Y)?;
            let az_deg = f64::from(hit.x.atan2(-hit.z).to_degrees())
                .rem_euclid(360.0)
                .round()
                .rem_euclid(360.0);
            if handle == GizmoHandle::AzAvoidStart {
                limits.az_avoid_start_deg = az_deg;
            } else {
                limits.az_avoid_end_deg = az_deg;
            }
        }
    }

    limits.sanitize();
    Some(limits)
}

fn paint_polyline(pts: &[Point<Pixels>], width: f32, color: Hsla, window: &mut Window) {
    if pts.len() < 2 || color.a <= 0.005 {
        return;
    }
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(pts[0]);
    for &p in &pts[1..] {
        path.line_to(p);
    }
    if let Ok(built) = path.build() {
        window.paint_path(built, color);
    }
}

fn paint_handle_dot(
    center: Point<Pixels>,
    color: Hsla,
    alpha: f32,
    is_active: bool,
    window: &mut Window,
) {
    if alpha <= 0.005 {
        return;
    }
    let r = if is_active { 5.5_f32 } else { 4.0_f32 };
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);

    // Subtle outer halo when hovered/dragged
    if is_active {
        paint_circle_fill(cx, cy, r + 4.5, color.opacity(alpha * 0.24), window);
    }
    // Crisp ring + filled core
    paint_circle_fill(cx, cy, r, color.opacity(alpha * 0.92), window);
    paint_circle_fill(
        cx,
        cy,
        (r - 1.8).max(1.2),
        Hsla::from(rgb(0x10141c)).opacity(alpha * 0.85),
        window,
    );
}

fn paint_circle_fill(cx: f32, cy: f32, radius: f32, color: Hsla, window: &mut Window) {
    const SEGMENTS: usize = 16;
    let mut path = PathBuilder::fill();
    for i in 0..SEGMENTS {
        let a = (i as f32 / SEGMENTS as f32) * TAU;
        let pt = point(px(cx + a.cos() * radius), px(cy + a.sin() * radius));
        if i == 0 {
            path.move_to(pt);
        } else {
            path.line_to(pt);
        }
    }
    path.close();
    if let Ok(built) = path.build() {
        window.paint_path(built, color);
    }
}

/// Paints all subtle proximity-revealed axis limit gizmos directly onto the GPUI canvas.
#[allow(clippy::too_many_arguments)]
pub fn paint_axis_gizmos(
    bounds: Bounds<Pixels>,
    cursor: Option<Point<Pixels>>,
    hovered: Option<GizmoHandle>,
    dragging: Option<GizmoHandle>,
    scene: &MountSceneState,
    camera: OrbitCamera,
    window: &mut Window,
) {
    // Completely invisible when the cursor is outside the viewport and not dragging
    let Some(cursor) =
        cursor.or_else(|| dragging.and_then(|h| handle_screen_position(h, scene, camera, bounds)))
    else {
        return;
    };

    let frames = AxisFrames::from_scene(scene);
    let dragging_group = dragging.map(GizmoHandle::axis_group);
    let base_track: Hsla = rgb(0xc8d4e6).into();

    // -------------------------------------------------------------------------
    // 1. Altitude Axis Arc (Min Alt + Max Alt)
    // -------------------------------------------------------------------------
    if scene.widgets.altitude_limits && dragging_group.is_none_or(|g| g == AxisGroup::Altitude) {
        let min_alt = (scene.limits.min_altitude_deg as f32).to_radians();
        let max_alt = (scene.limits.max_altitude_deg as f32).to_radians();
        let full_arc =
            sample_arc_segments(36, 0.0, FRAC_PI_2, camera, bounds, |a| frames.alt_point(a));
        let min_handle = project_world_point(frames.alt_point(min_alt), camera, bounds);
        let max_handle = project_world_point(frames.alt_point(max_alt), camera, bounds);

        let alpha = if dragging_group == Some(AxisGroup::Altitude) {
            1.0
        } else {
            let mut d = min_segments_dist(cursor, &full_arc);
            if let Some(p) = min_handle {
                d = d.min(dist_px(cursor, p));
            }
            if let Some(p) = max_handle {
                d = d.min(dist_px(cursor, p));
            }
            proximity_alpha(d)
        };

        if alpha > 0.01 {
            // Hairline full 0..90° altitude rail
            paint_arc_segments(&full_arc, 1.0, base_track.opacity(alpha * 0.24), window);

            // Min-altitude forbidden segment (warm amber)
            if min_alt > 0.01 {
                let seg =
                    sample_arc_segments(16, 0.0, min_alt, camera, bounds, |a| frames.alt_point(a));
                let active = hovered == Some(GizmoHandle::MinAltitude)
                    || dragging == Some(GizmoHandle::MinAltitude);
                let col: Hsla = rgb(0xf59447).into();
                paint_arc_segments(
                    &seg,
                    if active { 2.2 } else { 1.6 },
                    col.opacity(alpha * if active { 0.85 } else { 0.52 }),
                    window,
                );
            }

            // Max-altitude zenith cone segment (soft crimson)
            if max_alt < FRAC_PI_2 - 0.01 {
                let seg = sample_arc_segments(16, max_alt, FRAC_PI_2, camera, bounds, |a| {
                    frames.alt_point(a)
                });
                let active = hovered == Some(GizmoHandle::MaxAltitude)
                    || dragging == Some(GizmoHandle::MaxAltitude);
                let col: Hsla = rgb(0xf25c66).into();
                paint_arc_segments(
                    &seg,
                    if active { 2.2 } else { 1.6 },
                    col.opacity(alpha * if active { 0.85 } else { 0.52 }),
                    window,
                );
            }

            // Subtle current-altitude tick along the rail
            let cur_alt = (scene.alt_deg as f32).clamp(0.0, 90.0).to_radians();
            let (s, c) = cur_alt.sin_cos();
            let dir = frames.u_az_horiz * c + Vec3::Y * s;
            if let (Some(a), Some(b)) = (
                project_world_point(
                    frames.alt_center + dir * (ALT_ARC_RADIUS - 0.045),
                    camera,
                    bounds,
                ),
                project_world_point(
                    frames.alt_center + dir * (ALT_ARC_RADIUS + 0.045),
                    camera,
                    bounds,
                ),
            ) {
                let tick_col: Hsla = rgb(0x3dd9c7).into();
                paint_polyline(&[a, b], 1.5, tick_col.opacity(alpha * 0.72), window);
            }

            if let Some(p) = min_handle {
                paint_handle_dot(
                    p,
                    rgb(0xf59447).into(),
                    alpha,
                    hovered == Some(GizmoHandle::MinAltitude)
                        || dragging == Some(GizmoHandle::MinAltitude),
                    window,
                );
            }
            if let Some(p) = max_handle {
                paint_handle_dot(
                    p,
                    rgb(0xf25c66).into(),
                    alpha,
                    hovered == Some(GizmoHandle::MaxAltitude)
                        || dragging == Some(GizmoHandle::MaxAltitude),
                    window,
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // 2. RA Polar Ring (Meridian Flip Limit)
    // -------------------------------------------------------------------------
    if scene.widgets.meridian_flip && dragging_group.is_none_or(|g| g == AxisGroup::Meridian) {
        let mer_lim = (scene.limits.meridian_limit_deg as f32)
            .clamp(0.0, 45.0)
            .to_radians();
        let ring = sample_arc_segments(40, -FRAC_PI_2, FRAC_PI_2, camera, bounds, |h| {
            frames.ra_point(h)
        });
        let mer_handle = project_world_point(frames.ra_point(mer_lim), camera, bounds);

        let alpha = if dragging_group == Some(AxisGroup::Meridian) {
            1.0
        } else {
            let mut d = min_segments_dist(cursor, &ring);
            if let Some(p) = mer_handle {
                d = d.min(dist_px(cursor, p));
            }
            proximity_alpha(d)
        };

        if alpha > 0.01 {
            // Subtle RA ring arc around the polar collar
            paint_arc_segments(&ring, 1.0, base_track.opacity(alpha * 0.24), window);

            // Meridian 0° reference tick
            if let (Some(a), Some(b)) = (
                project_world_point(
                    frames.ra_center + frames.u_meridian * (RA_RING_RADIUS - 0.04),
                    camera,
                    bounds,
                ),
                project_world_point(
                    frames.ra_center + frames.u_meridian * (RA_RING_RADIUS + 0.04),
                    camera,
                    bounds,
                ),
            ) {
                paint_polyline(&[a, b], 1.2, base_track.opacity(alpha * 0.55), window);
            }

            // Meridian limit wedge arc (0° -> +meridian_limit_deg)
            let active = hovered == Some(GizmoHandle::MeridianLimit)
                || dragging == Some(GizmoHandle::MeridianLimit);
            let col: Hsla = rgb(0x59b8fa).into();
            if mer_lim > 0.008 {
                let seg =
                    sample_arc_segments(18, 0.0, mer_lim, camera, bounds, |h| frames.ra_point(h));
                paint_arc_segments(
                    &seg,
                    if active { 2.2 } else { 1.6 },
                    col.opacity(alpha * if active { 0.88 } else { 0.56 }),
                    window,
                );
            }

            // Current hour-angle indicator tick on the RA ring
            let cur_ha = (scene.hour_angle_deg() as f32).to_radians();
            if cur_ha.abs() <= PI * 0.65 {
                let (s, c) = cur_ha.sin_cos();
                let dir = frames.u_meridian * c + frames.u_west * s;
                if let (Some(a), Some(b)) = (
                    project_world_point(
                        frames.ra_center + dir * (RA_RING_RADIUS - 0.035),
                        camera,
                        bounds,
                    ),
                    project_world_point(
                        frames.ra_center + dir * (RA_RING_RADIUS + 0.035),
                        camera,
                        bounds,
                    ),
                ) {
                    let tick_col: Hsla = rgb(0x3dd9c7).into();
                    paint_polyline(&[a, b], 1.5, tick_col.opacity(alpha * 0.72), window);
                }
            }

            if let Some(p) = mer_handle {
                paint_handle_dot(p, col, alpha, active, window);
            }
        }
    }

    // -------------------------------------------------------------------------
    // 3. Azimuth Ground Ring (Exclusion Sector Start & End, when enabled)
    // -------------------------------------------------------------------------
    if scene.widgets.azimuth_sector
        && scene.limits.az_avoid_enabled
        && dragging_group.is_none_or(|g| g == AxisGroup::Azimuth)
    {
        let start_deg = scene.limits.az_avoid_start_deg as f32;
        let end_deg = scene.limits.az_avoid_end_deg as f32;
        let sweep_deg = (end_deg - start_deg).rem_euclid(360.0);
        let az_start = start_deg.to_radians();
        let az_end = (start_deg + sweep_deg).to_radians();

        let sector_arc =
            sample_arc_segments(32, az_start, az_end, camera, bounds, AxisFrames::az_point);
        let start_handle = project_world_point(AxisFrames::az_point(az_start), camera, bounds);
        let end_handle = project_world_point(AxisFrames::az_point(az_end), camera, bounds);

        let alpha = if dragging_group == Some(AxisGroup::Azimuth) {
            1.0
        } else {
            let mut d = min_segments_dist(cursor, &sector_arc);
            if let Some(p) = start_handle {
                d = d.min(dist_px(cursor, p));
            }
            if let Some(p) = end_handle {
                d = d.min(dist_px(cursor, p));
            }
            proximity_alpha(d)
        };

        if alpha > 0.01 {
            let col: Hsla = rgb(0xf25c66).into();
            let active_any = matches!(
                hovered.or(dragging),
                Some(GizmoHandle::AzAvoidStart | GizmoHandle::AzAvoidEnd)
            );
            paint_arc_segments(
                &sector_arc,
                if active_any { 2.2 } else { 1.6 },
                col.opacity(alpha * if active_any { 0.85 } else { 0.52 }),
                window,
            );
            if let Some(p) = start_handle {
                paint_handle_dot(
                    p,
                    col,
                    alpha,
                    hovered == Some(GizmoHandle::AzAvoidStart)
                        || dragging == Some(GizmoHandle::AzAvoidStart),
                    window,
                );
            }
            if let Some(p) = end_handle {
                paint_handle_dot(
                    p,
                    col,
                    alpha,
                    hovered == Some(GizmoHandle::AzAvoidEnd)
                        || dragging == Some(GizmoHandle::AzAvoidEnd),
                    window,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn test_bounds() -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(1200.0), px(800.0)))
    }

    #[test]
    fn project_and_unproject_round_trip_across_wide_stereographic_fov() {
        let bounds = test_bounds();
        for fov_y_deg in [42.0_f32, 85.0, 140.0] {
            let camera = OrbitCamera {
                fov_y_deg,
                ..OrbitCamera::default()
            };
            let world_pt = Vec3::new(0.4, 1.5, -0.3);
            let screen = project_world_point(world_pt, camera, bounds).expect("visible point");
            let (origin, dir) = ray_from_screen(screen, camera, bounds).expect("valid ray");
            let expected_dir = (world_pt - origin).normalize();
            assert!(
                dir.dot(expected_dir) > 0.9999,
                "ray mismatch at fov={fov_y_deg}: dot={}",
                dir.dot(expected_dir)
            );
        }
    }

    #[test]
    fn dragging_altitude_and_meridian_handles_updates_limits_accurately() {
        let bounds = test_bounds();
        let camera = OrbitCamera::default();
        let scene = MountSceneState::default();
        let frames = AxisFrames::from_scene(&scene);

        // Drag MinAltitude handle to 25°
        let pt_25 = project_world_point(frames.alt_point(25.0_f32.to_radians()), camera, bounds)
            .expect("alt 25 visible");
        let updated =
            drag_handle_to_limits(GizmoHandle::MinAltitude, pt_25, &scene, camera, bounds)
                .expect("min alt drag");
        assert!((updated.min_altitude_deg - 25.0).abs() <= 1.0);

        // Drag MeridianLimit handle to +18°
        let pt_18 = project_world_point(frames.ra_point(18.0_f32.to_radians()), camera, bounds)
            .expect("ha 18 visible");
        let updated_mer =
            drag_handle_to_limits(GizmoHandle::MeridianLimit, pt_18, &scene, camera, bounds)
                .expect("meridian drag");
        assert!((updated_mer.meridian_limit_deg - 18.0).abs() <= 1.0);
    }
}
