#import planetarium::celestial_projection::{CelestialProjectionUniforms, unproject_celestial}

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;

struct CelestialLandscapeUniforms {
    camera_to_topo: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    // x = opacity in [0, 1], y = az_offset_rad, z = has_texture (0 or 1), w = sample_count (f32)
    params: vec4<f32>,
    // rgb = linear lighting multiplier adapted to solar altitude, w = horizon line accent alpha
    lighting_and_line: vec4<f32>,
    // rgb = linear RGB silhouette horizon color, w = unused
    silhouette_horizon_color: vec4<f32>,
    // rgb = linear RGB silhouette nadir color, w = unused
    silhouette_nadir_color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialLandscapeUniforms;
@group(0) @binding(1) var<storage, read> horizon_samples: array<f32>;
@group(0) @binding(2) var panorama_texture: texture_2d<f32>;
@group(0) @binding(3) var panorama_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) vid: u32) -> VertexOutput {
    let x = f32(i32(vid & 1u) * 4 - 1);
    let y = f32(i32(vid & 2u) * 2 - 1);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.ndc = vec2<f32>(x, y);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let opacity = uniforms.params.x;
    let count = u32(max(uniforms.params.w, 1.0));
    if (opacity <= 1e-4) {
        discard;
    }

    let v_cam = unproject_celestial(input.ndc, uniforms.projection);
    let dir_enu = normalize((uniforms.camera_to_topo * vec4<f32>(v_cam, 0.0)).xyz);

    var az_rad = atan2(dir_enu.x, dir_enu.y);
    if (az_rad < 0.0) {
        az_rad += TAU;
    }
    let alt_rad = asin(clamp(dir_enu.z, -1.0, 1.0));

    let img_az_frac = fract((az_rad - uniforms.params.y) / TAU);
    let u_img = img_az_frac;
    let v_img = clamp(0.5 - alt_rad / PI, 0.0, 1.0);

    let pos = img_az_frac * f32(count);
    let i0 = u32(floor(pos)) % count;
    let i1 = (i0 + 1u) % count;
    let frac = pos - floor(pos);
    let h_rad = mix(horizon_samples[i0], horizon_samples[i1], frac);

    let diff = alt_rad - h_rad;
    // Compute derivatives and texture fetch in uniform control flow before per-pixel branches.
    let diff_fw = fwidth(diff);
    let aa = clamp(diff_fw, 0.0005, 0.025);
    let line_half_w = max(clamp(diff_fw, 0.0005, 0.008) * 1.35, 0.0012);
    let tex_sample = textureSampleLevel(panorama_texture, panorama_sampler, vec2<f32>(u_img, v_img), 0.0);

    let below_mask = 1.0 - smoothstep(-aa, aa, diff);
    var terrain_rgb = vec3<f32>(0.0);
    var terrain_a = 0.0;
    if (below_mask > 1e-4) {
        if (uniforms.params.z > 0.5) {
            terrain_rgb = tex_sample.rgb * uniforms.lighting_and_line.rgb;
            terrain_a = below_mask * tex_sample.a * opacity;
        } else {
            let depth = clamp(-diff / 1.2, 0.0, 1.0);
            terrain_rgb = mix(
                uniforms.silhouette_horizon_color.rgb,
                uniforms.silhouette_nadir_color.rgb,
                depth,
            );
            terrain_a = below_mask * opacity;
        }
    }

    let line_alpha = uniforms.lighting_and_line.w * opacity;
    let line_mask = (1.0 - smoothstep(0.0, line_half_w, abs(diff))) * line_alpha;
    let line_rgb = vec3<f32>(0.05, 0.56, 0.72);

    // Porter-Duff: horizon outline OVER terrain in premultiplied alpha.
    let out_rgb = line_rgb * line_mask + terrain_rgb * terrain_a * (1.0 - line_mask);
    let out_a = line_mask + terrain_a * (1.0 - line_mask);
    if (out_a <= 1e-4 && max(out_rgb.r, max(out_rgb.g, out_rgb.b)) <= 1e-6) {
        discard;
    }

    return vec4<f32>(out_rgb, out_a);
}
