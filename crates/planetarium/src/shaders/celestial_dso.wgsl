#import planetarium::celestial_projection::{CelestialProjectionUniforms, project_celestial}

struct CelestialDsoUniforms {
    transform: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    viewport_size: vec2<f32>,
    ngc_limiting_mag: f32,
    messier_boost: f32,
    fade_range: f32,
    opacity: f32,
    catalog_mask: u32,
    atmo_visibility: f32,
}

struct DsoGpuRecord {
    pos: vec3<f32>,
    eff_mag: f32,
    semi_major_rad: f32,
    semi_minor_rad: f32,
    pos_angle_rad: f32,
    packed_flags: u32,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialDsoUniforms;
@group(0) @binding(1) var<storage, read> dsos: array<DsoGpuRecord>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_px: vec2<f32>,
    @location(1) radii_px: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) alpha: f32,
    @location(4) @interpolate(flat) symbol_and_color: u32,
}

fn dso_srgb(color_id: u32) -> vec3<f32> {
    switch (color_id) {
        case 0u: { return vec3<f32>(0.92, 0.44, 0.42); }
        case 1u: { return vec3<f32>(0.95, 0.80, 0.36); }
        case 2u: { return vec3<f32>(0.96, 0.68, 0.28); }
        case 3u: { return vec3<f32>(0.30, 0.86, 0.88); }
        case 4u: { return vec3<f32>(0.35, 0.85, 0.56); }
        case 5u: { return vec3<f32>(0.45, 0.72, 0.96); }
        case 6u: { return vec3<f32>(0.62, 0.66, 0.74); }
        case 7u: { return vec3<f32>(0.46, 0.88, 0.66); }
        default: { return vec3<f32>(0.72, 0.66, 0.92); }
    }
}

fn culled_vertex() -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    out.local_px = vec2<f32>(0.0);
    out.radii_px = vec2<f32>(1.0);
    out.color = vec3<f32>(0.0);
    out.alpha = 0.0;
    out.symbol_and_color = 0u;
    return out;
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    let dso_index = input.instance_index;
    if (dso_index >= arrayLength(&dsos)) {
        return culled_vertex();
    }

    let dso = dsos[dso_index];
    let symbol = dso.packed_flags & 0xFFu;
    let color_id = (dso.packed_flags >> 8u) & 0xFFu;
    let is_messier = (dso.packed_flags & (1u << 16u)) != 0u;
    let is_ngc = (dso.packed_flags & (1u << 17u)) != 0u;

    let show_messier = (uniforms.catalog_mask & 1u) != 0u;
    let show_ngc = (uniforms.catalog_mask & 2u) != 0u;
    let visible_in_catalog = (is_messier && show_messier) || (is_ngc && show_ngc);
    if (!visible_in_catalog) {
        return culled_vertex();
    }

    let v_cam = (uniforms.transform * vec4<f32>(dso.pos, 0.0)).xyz;
    let v_len = length(v_cam);
    if (v_len <= 1e-6) {
        return culled_vertex();
    }
    let v_dir = v_cam / v_len;
    let min_z = mix(-0.98, 0.02, uniforms.projection.blend_factor);
    if (v_dir.z <= min_z) {
        return culled_vertex();
    }

    var north_icrs: vec3<f32>;
    if (abs(dso.pos.z) < 0.9999) {
        north_icrs = normalize(vec3<f32>(0.0, 0.0, 1.0) - dso.pos * dso.pos.z);
    } else {
        north_icrs = normalize(vec3<f32>(1.0, 0.0, 0.0) - dso.pos * dso.pos.x);
    }
    let east_icrs = normalize(cross(north_icrs, dso.pos));
    let major_icrs = normalize(
        north_icrs * cos(dso.pos_angle_rad) + east_icrs * sin(dso.pos_angle_rad),
    );
    let major_cam = (uniforms.transform * vec4<f32>(major_icrs, 0.0)).xyz;

    let screen_dim = max(uniforms.viewport_size, vec2<f32>(1.0, 1.0));
    let center_clip = project_celestial(v_dir, uniforms.projection);

    let probe_eps = 0.002;
    let probe_dir = normalize(v_dir + major_cam * probe_eps);
    let probe_clip = project_celestial(probe_dir, uniforms.projection);
    let delta_px = (probe_clip.xy - center_clip.xy) * (0.5 * screen_dim);
    let delta_len = length(delta_px);
    let px_per_rad = delta_len / probe_eps;
    let u_dir_px = select(vec2<f32>(0.0, 1.0), delta_px / delta_len, delta_len > 1e-5);
    let v_dir_px = vec2<f32>(-u_dir_px.y, u_dir_px.x);

    let true_a_px = dso.semi_major_rad * px_per_rad;
    let max_dim = max(screen_dim.x, screen_dim.y);
    let min_dim = min(screen_dim.x, screen_dim.y);
    if (true_a_px > 4.0 * max_dim) {
        return culled_vertex();
    }

    let limit = uniforms.ngc_limiting_mag + select(0.0, uniforms.messier_boost, is_messier);
    let fade_range = max(uniforms.fade_range, 0.1);
    let mag_vis = clamp((limit + fade_range - dso.eff_mag) / fade_range, 0.0, 1.0);
    let size_vis = smoothstep(8.0, 18.0, true_a_px);
    let base_vis = max(mag_vis, size_vis);

    let oversize_t = smoothstep(0.45 * min_dim, 1.8 * min_dim, true_a_px);
    let giant_atten = mix(1.0, 0.32, oversize_t);
    let base_alpha = select(0.76, 0.90, is_messier);
    let total_alpha = base_vis
        * giant_atten
        * base_alpha
        * clamp(uniforms.opacity, 0.0, 1.0)
        * clamp(uniforms.atmo_visibility, 0.0, 1.0);

    if (total_alpha <= 1e-3) {
        return culled_vertex();
    }

    let min_icon_r = select(6.0, 7.5, is_messier);
    let disp_a = max(true_a_px, min_icon_r);
    var disp_b = disp_a;
    if (symbol == 0u && dso.semi_major_rad > 1e-6) {
        let raw_ratio = select(
            1.0,
            clamp(dso.semi_minor_rad / dso.semi_major_rad, 0.18, 1.0),
            dso.semi_minor_rad > 0.0,
        );
        let t_true = smoothstep(min_icon_r, min_icon_r * 2.2, true_a_px);
        let eff_ratio = mix(max(raw_ratio, 0.48), raw_ratio, t_true);
        disp_b = max(disp_a * eff_ratio, 3.5);
    }

    let pad_px = 4.0;
    let ext_ndc = (disp_a + pad_px) * (2.0 / min_dim);
    if (abs(center_clip.x) > 1.0 + ext_ndc + 0.05 || abs(center_clip.y) > 1.0 + ext_ndc + 0.05) {
        return culled_vertex();
    }

    let corner = input.position.xy;
    let local_px = vec2<f32>(corner.x * (disp_a + pad_px), corner.y * (disp_b + pad_px));
    let offset_px = u_dir_px * local_px.x + v_dir_px * local_px.y;
    let ndc_offset = offset_px * (2.0 / screen_dim);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        center_clip.x + ndc_offset.x * center_clip.w,
        center_clip.y + ndc_offset.y * center_clip.w,
        center_clip.z,
        center_clip.w,
    );
    out.local_px = local_px;
    out.radii_px = vec2<f32>(disp_a, disp_b);
    out.color = pow(max(dso_srgb(color_id), vec3<f32>(0.0)), vec3<f32>(2.2));
    out.alpha = total_alpha;
    out.symbol_and_color = symbol | (color_id << 8u);
    return out;
}

fn stroke_alpha(dist: f32, half_w: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(half_w, half_w + aa, dist);
}

fn dash_factor(p: vec2<f32>, segments: f32) -> f32 {
    let angle = atan2(p.y, p.x);
    let phase = fract(((angle + 3.14159265) / 6.28318531) * segments);
    return smoothstep(0.08, 0.16, phase) * (1.0 - smoothstep(0.60, 0.68, phase));
}

fn sd_rounded_box(p: vec2<f32>, half_extent: f32, corner_r: f32) -> f32 {
    let q = abs(p) - vec2<f32>(half_extent - corner_r, half_extent - corner_r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - corner_r;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let p = input.local_px;
    let ra = max(input.radii_px.x, 1.0);
    let rb = max(input.radii_px.y, 1.0);
    let symbol = input.symbol_and_color & 0xFFu;
    let color_id = (input.symbol_and_color >> 8u) & 0xFFu;

    let half_w = 0.72;
    let aa = 0.85;
    var cov = 0.0;

    switch (symbol) {
        // 0: Galaxy — tilted ellipse outline
        case 0u: {
            let q = p / vec2<f32>(ra, rb);
            let k0 = length(q);
            let k1 = length(p / vec2<f32>(ra * ra, rb * rb));
            let d_ellipse = abs(k0 * (k0 - 1.0)) / max(k1, 1e-6);
            cov = stroke_alpha(d_ellipse, half_w, aa);
        }
        // 1: Open Cluster / Star Association — 12-segment dashed circle
        case 1u: {
            let d_circle = abs(length(p) - ra);
            cov = stroke_alpha(d_circle, half_w, aa) * dash_factor(p, 12.0);
        }
        // 2: Globular Cluster — solid circle + internal crosshair '+'
        case 2u: {
            let d_circle = abs(length(p) - ra);
            let cross_r = ra * 0.90;
            let d_h = max(abs(p.y), abs(p.x) - cross_r);
            let d_v = max(abs(p.x), abs(p.y) - cross_r);
            let d_cross = min(d_h, d_v);
            cov = max(stroke_alpha(d_circle, half_w, aa), stroke_alpha(d_cross, 0.56, aa) * 0.82);
        }
        // 3: Planetary Nebula — circle + 4 external ticks
        case 3u: {
            let r_in = ra * 0.72;
            let d_circle = abs(length(p) - r_in);
            let tick_out = ra * 1.15;
            let tick_mid = 0.5 * (r_in + tick_out);
            let tick_half = 0.5 * (tick_out - r_in);
            let d_tx = max(abs(p.y), abs(abs(p.x) - tick_mid) - tick_half);
            let d_ty = max(abs(p.x), abs(abs(p.y) - tick_mid) - tick_half);
            cov = max(stroke_alpha(d_circle, half_w, aa), stroke_alpha(min(d_tx, d_ty), half_w, aa));
        }
        // 4: Diffuse / HII / Emission / Reflection / Dark Nebula — rounded square (dashed for Dark)
        case 4u: {
            let s = ra * 0.88;
            let cr = min(s * 0.24, 5.0);
            let d_box = abs(sd_rounded_box(p, s, cr));
            cov = stroke_alpha(d_box, half_w, aa);
            if (color_id == 6u) {
                cov *= dash_factor(p, 12.0);
            }
        }
        // 5: Cluster + Nebula — outer rounded square + inner dashed circle
        case 5u: {
            let s = ra * 0.92;
            let cr = min(s * 0.24, 5.0);
            let d_box = abs(sd_rounded_box(p, s, cr));
            let d_in = abs(length(p) - ra * 0.56);
            let box_cov = stroke_alpha(d_box, half_w, aa);
            let in_cov = stroke_alpha(d_in, 0.58, aa) * dash_factor(p, 10.0);
            cov = max(box_cov, in_cov);
        }
        // 6: Supernova Remnant — dashed diamond ring
        case 6u: {
            let d_diamond = abs((abs(p.x) + abs(p.y)) * 0.70710678 - ra * 0.92);
            cov = stroke_alpha(d_diamond, half_w, aa) * dash_factor(p, 12.0);
        }
        // 7: Double Star / Star / Other — crisp small diamond
        default: {
            let d_diamond = abs((abs(p.x) + abs(p.y)) * 0.70710678 - ra * 0.85);
            cov = stroke_alpha(d_diamond, half_w, aa);
        }
    }

    let final_alpha = clamp(cov * input.alpha, 0.0, 1.0);
    if (final_alpha <= 1e-3) {
        discard;
    }

    return vec4<f32>(input.color, final_alpha);
}
