struct CameraUniforms {
    view: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // [aspect_ratio, blend_factor, scale_factor, fov_y_rad]
    // Matches `CameraProjection::for_fov` (0.0 = stereographic, 1.0 = perspective).
    proj_params: vec4<f32>,
    // Precomputed per-frame solar/sky lighting uniforms (saves per-fragment smoothstep/mix work):
    // xyz = normalized sun direction (+X East, +Y Up, -Z North), w = sun_intensity
    sun_dir_intensity: vec4<f32>,
    // xyz = direct sun color, w = night_fill_weight (1 - day_t)
    sun_color_night_w: vec4<f32>,
    // xyz = normalized twilight horizon direction, w = twilight_weight
    twilight_dir_weight: vec4<f32>,
    // xyz = sky hemisphere ambient color
    sky_ambient: vec4<f32>,
    // xyz = ground hemisphere ambient color
    gnd_ambient: vec4<f32>,
    // xyz = Fresnel rim color
    rim_color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> camera: CameraUniforms;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) cam_z: f32,
}

fn safe_normalize(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let len_sq = dot(v, v);
    if (len_sq > 1e-10) {
        return v * inverseSqrt(len_sq);
    }
    return fallback;
}

// Projects a camera-space vector (+X right, +Y up, +Z forward) using the exact same
// stereographic-to-perspective blended projection as `planetarium::celestial_projection`.
fn project_camera_space(v_cam: vec3<f32>, z_ndc: f32) -> vec4<f32> {
    let norm = safe_normalize(v_cam, vec3<f32>(0.0, 0.0, 1.0));
    let nz = clamp(norm.z, -1.0, 1.0);
    let stereo_factor = 2.0 / max(1.0 + nz, 0.002);
    let persp_factor = 1.0 / max(nz, 0.005);
    let factor = mix(stereo_factor, persp_factor, camera.proj_params.y);
    let aspect = max(camera.proj_params.x, 0.1);
    let scale = max(camera.proj_params.z, 1e-6);
    let ndc_x = (norm.x * factor) / (aspect * scale);
    let ndc_y = (norm.y * factor) / scale;
    return vec4<f32>(ndc_x, ndc_y, z_ndc, 1.0);
}

fn foreground_depth(dist: f32) -> f32 {
    let near_plane = 0.05;
    let far_plane = 100.0;
    return clamp(
        (far_plane * (dist - near_plane)) / (max(dist, near_plane) * (far_plane - near_plane)),
        0.0,
        0.998,
    );
}

@vertex
fn vs_lit(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let p_view = (camera.view * vec4<f32>(in.position, 1.0)).xyz;
    let v_cam = vec3<f32>(p_view.x, p_view.y, -p_view.z);
    let dist = length(v_cam);
    out.clip_position = project_camera_space(v_cam, foreground_depth(dist));
    out.world_pos = in.position;
    out.world_normal = in.normal;
    out.color = in.color;
    out.cam_z = select(0.0, v_cam.z / dist, dist > 1e-6);
    return out;
}

@fragment
fn fs_lit(in: VertexOut) -> @location(0) vec4<f32> {
    let min_z = mix(-0.98, 0.02, camera.proj_params.y);
    if (in.cam_z < min_z) {
        discard;
    }

    let n = safe_normalize(in.world_normal, vec3<f32>(0.0, 1.0, 0.0));
    let v = safe_normalize(camera.camera_pos.xyz - in.world_pos, vec3<f32>(0.0, 1.0, 0.0));

    let sun_dir = camera.sun_dir_intensity.xyz;
    let sun_intensity = camera.sun_dir_intensity.w;
    let sun_color = camera.sun_color_night_w.xyz;
    let night_w = camera.sun_color_night_w.w;

    // Direct Sun key light
    let ndl_sun = max(dot(n, sun_dir), 0.0);
    let direct_sun = sun_color * (ndl_sun * sun_intensity * 0.92);

    // Directional twilight glow from Sun azimuth
    let twilight_bounce = vec3<f32>(0.42, 0.24, 0.18)
        * (max(dot(n, camera.twilight_dir_weight.xyz), 0.0) * camera.twilight_dir_weight.w);

    // Hemisphere sky + ground bounce ambient
    let hemi = n.y * 0.5 + 0.5;
    let ambient = mix(camera.gnd_ambient.xyz, camera.sky_ambient.xyz, hemi);

    // Cool night-sky overhead fill (pre-normalized direction)
    let night_fill_dir = vec3<f32>(-0.251754, 0.926455, 0.281965);
    let night_fill = vec3<f32>(0.16, 0.22, 0.34) * (max(dot(n, night_fill_dir), 0.0) * night_w);

    // Specular highlight from Sun (guarded against v == -sun_dir NaN) + Fresnel rim
    let half_vec = safe_normalize(sun_dir + v, n);
    let sun_spec = sun_color * (pow(max(dot(n, half_vec), 0.0), 40.0) * 0.30 * sun_intensity);
    let fresnel = pow(1.0 - max(dot(n, v), 0.0), 3.0);
    let rim = camera.rim_color.xyz * (fresnel * (0.14 + 0.14 * hemi));

    let lit = in.color.rgb * (ambient + direct_sun + twilight_bounce + night_fill) + sun_spec + rim;

    return vec4<f32>(lit, in.color.a);
}

@vertex
fn vs_line(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let p_view = (camera.view * vec4<f32>(in.position, 1.0)).xyz;
    let v_cam = vec3<f32>(p_view.x, p_view.y, -p_view.z);
    let dist = length(v_cam);
    out.clip_position = project_camera_space(v_cam, foreground_depth(dist));
    out.world_pos = in.position;
    out.world_normal = in.normal;
    out.color = in.color;
    out.cam_z = select(0.0, v_cam.z / dist, dist > 1e-6);
    return out;
}

// Projects vertices at infinity (direction-only, zero camera translation) so HUD limit zones,
// meridian plane, and optical-axis sky reticle exhibit zero parallax against the Planetarium sky.
@vertex
fn vs_infinity(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let p_view = (camera.view * vec4<f32>(in.position, 0.0)).xyz;
    let v_cam = vec3<f32>(p_view.x, p_view.y, -p_view.z);
    let dist = length(v_cam);
    out.clip_position = project_camera_space(v_cam, 0.999);
    out.world_pos = in.position + camera.camera_pos.xyz;
    out.world_normal = in.normal;
    out.color = in.color;
    out.cam_z = select(0.0, v_cam.z / dist, dist > 1e-6);
    return out;
}

@fragment
fn fs_unlit(in: VertexOut) -> @location(0) vec4<f32> {
    let min_z = mix(-0.98, 0.02, camera.proj_params.y);
    if (in.cam_z < min_z) {
        discard;
    }
    return in.color;
}
