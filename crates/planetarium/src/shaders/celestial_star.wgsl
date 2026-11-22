#import planetarium::celestial_projection::{CelestialProjectionUniforms, project_celestial}

struct CelestialStarUniforms {
    transform: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    viewport_size: vec2<f32>,
    limiting_mag: f32,
    fade_range: f32,
    base_radius: f32,
    min_radius: f32,
    max_radius: f32,
    opacity: f32,
    zenith_and_atmo: vec4<f32>,
    sun_cam_and_alt: vec4<f32>,
}

struct StarRecord {
    pos: vec3<f32>,
    packed_props: u32,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialStarUniforms;
@group(0) @binding(1) var<storage, read> stars: array<StarRecord>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) alpha: f32,
}

// Thank you Gemini
fn bp_rp_to_srgb(bp_rp: f32) -> vec3<f32> {
    let t = clamp((bp_rp + 0.3) / 2.5, 0.0, 1.0);
    if (t < 0.15) {
        let u = t / 0.15;
        return mix(vec3<f32>(0.65, 0.78, 1.0), vec3<f32>(0.85, 0.90, 1.0), u);
    } else if (t < 0.35) {
        let u = (t - 0.15) / 0.20;
        return mix(vec3<f32>(0.85, 0.90, 1.0), vec3<f32>(1.0, 1.0, 0.98), u);
    } else if (t < 0.55) {
        let u = (t - 0.35) / 0.20;
        return mix(vec3<f32>(1.0, 1.0, 0.98), vec3<f32>(1.0, 0.92, 0.75), u);
    } else if (t < 0.75) {
        let u = (t - 0.55) / 0.20;
        return mix(vec3<f32>(1.0, 0.92, 0.75), vec3<f32>(1.0, 0.75, 0.45), u);
    } else {
        let u = (t - 0.75) / 0.25;
        return mix(vec3<f32>(1.0, 0.75, 0.45), vec3<f32>(1.0, 0.50, 0.30), u);
    }
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let star_index = input.instance_index;
    if (star_index >= arrayLength(&stars)) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.color = vec3<f32>(0.0, 0.0, 0.0);
        out.alpha = 0.0;
        return out;
    }

    let star = stars[star_index];

    let mag_raw = i32(star.packed_props & 0xFFFFu);
    let mag_i16 = (mag_raw << 16) >> 16;
    let mag = f32(mag_i16) * 0.01;

    let fov = uniforms.projection.fov_y;
    let zoom_factor = clamp(pow(1.8 / max(fov, 0.005), 0.28), 0.85, 4.5);

    let mag_factor = pow(10.0, -0.075 * (mag - 3.5));
    let base_r = uniforms.base_radius * zoom_factor;
    let min_r = uniforms.min_radius * clamp(zoom_factor * 0.9, 0.75, 2.5);
    let max_r = uniforms.max_radius * (1.0 + (zoom_factor - 1.0) * 0.6);
    let radius_px = clamp(base_r * mag_factor, min_r, max_r);

    let mag_brightness = clamp(pow(10.0, -0.15 * max(mag - 3.0, 0.0)), 0.22, 1.0);
    let alpha_zoom_boost = clamp((zoom_factor - 1.0) * 0.35, 0.0, 0.65);
    let star_alpha = mix(mag_brightness, 1.0, alpha_zoom_boost);

    // Fast-path cull before projection: extinction & solar glare only add >= 0 to mag.
    if (mag - uniforms.limiting_mag >= uniforms.fade_range) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.color = vec3<f32>(0.0, 0.0, 0.0);
        out.alpha = 0.0;
        return out;
    }

    let v_cam = (uniforms.transform * vec4<f32>(star.pos, 0.0)).xyz;
    let v_len = length(v_cam);
    let v_dir = select(vec3<f32>(0.0, 0.0, 1.0), v_cam / v_len, v_len > 1e-6);
    let v_cam_z = select(0.0, v_dir.z, v_len > 1e-6);
    let min_z = mix(-0.98, 0.02, uniforms.projection.blend_factor);

    if (v_cam_z <= min_z) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.color = vec3<f32>(0.0, 0.0, 0.0);
        out.alpha = 0.0;
        return out;
    }

    // Directional horizon extinction + solar twilight/daytime angular proximity glare
    let atmo_strength = clamp(uniforms.zenith_and_atmo.w, 0.0, 1.0);
    let sin_alt = dot(v_dir, uniforms.zenith_and_atmo.xyz);
    let airmass = 1.0 / max(sin_alt + 0.035, 0.035);
    let ext_mag = 0.16 * max(airmass - 1.0, 0.0) * atmo_strength;

    let cos_sun = dot(v_dir, uniforms.sun_cam_and_alt.xyz);
    let sun_alt_deg = uniforms.sun_cam_and_alt.w;
    let tw_glare = smoothstep(-16.0, 0.0, sun_alt_deg) * atmo_strength;
    let sun_pos = max(cos_sun, 0.0);
    let sun_pos2 = sun_pos * sun_pos;
    let sun_pos4 = sun_pos2 * sun_pos2;
    let sun_pos8 = sun_pos4 * sun_pos4;
    let glare_mag = tw_glare * (1.1 * sun_pos2 + 2.2 * sun_pos8);

    let effective_mag = mag + ext_mag + glare_mag;
    var mag_cutoff_alpha = 1.0;
    let mag_diff = effective_mag - uniforms.limiting_mag;
    if (mag_diff > 0.0) {
        if (mag_diff >= uniforms.fade_range) {
            out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
            out.uv = vec2<f32>(0.0, 0.0);
            out.color = vec3<f32>(0.0, 0.0, 0.0);
            out.alpha = 0.0;
            return out;
        }
        mag_cutoff_alpha = 1.0 - (mag_diff / uniforms.fade_range);
    }
    let contrast_fade = clamp(
        (uniforms.limiting_mag + uniforms.fade_range - effective_mag)
            / max(uniforms.fade_range + 1.5 * tw_glare, 0.6),
        0.0,
        1.0,
    );
    let total_alpha =
        star_alpha * mag_cutoff_alpha * contrast_fade * clamp(uniforms.opacity, 0.0, 1.0);

    let center_clip = project_celestial(v_cam, uniforms.projection);

    let ndc_margin = 0.25;
    if (abs(center_clip.x) > 1.0 + ndc_margin || abs(center_clip.y) > 1.0 + ndc_margin) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.color = vec3<f32>(0.0, 0.0, 0.0);
        out.alpha = 0.0;
        return out;
    }

    let color_raw = i32(star.packed_props >> 16u);
    let color_i16 = (color_raw << 16) >> 16;
    let bp_rp = f32(color_i16) * 0.01;

    let corner = input.position.xy;
    let screen_dim = max(uniforms.viewport_size, vec2<f32>(1.0, 1.0));
    let ndc_offset = corner * (radius_px * 2.0 / screen_dim);

    out.clip_position = vec4<f32>(
        center_clip.x + ndc_offset.x * center_clip.w,
        center_clip.y + ndc_offset.y * center_clip.w,
        center_clip.z,
        center_clip.w
    );
    out.uv = corner;
    out.color = pow(max(bp_rp_to_srgb(bp_rp), vec3<f32>(0.0)), vec3<f32>(2.2));
    out.alpha = total_alpha;
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let r2 = dot(input.uv, input.uv);
    if (r2 > 1.0) {
        discard;
    }

    let r = sqrt(r2);

    let core = exp(-18.0 * r2);

    let disk = exp(-4.2 * r2);

    let halo = pow(max(0.0, 1.0 - r), 2.2) * 0.45;

    let intensity = max(0.0, (mix(disk, 1.0, core * 0.5) + halo) / 1.45);
    let final_alpha = clamp(intensity * input.alpha, 0.0, 1.0);

    // White core saturation for realism
    let star_color = mix(input.color, vec3<f32>(1.0, 1.0, 1.0), core * 0.85);

    return vec4<f32>(star_color, final_alpha);
}
