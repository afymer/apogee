#import planetarium::celestial_projection::{CelestialProjectionUniforms, unproject_celestial}

struct CelestialSkyUniforms {
    camera_to_topo: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    // xyz = unit topocentric ENU Sun direction, w = effective atmosphere strength in [0.0, 1.0]
    sun_dir_and_atmo_strength: vec4<f32>,
    // Linear RGB zenith sky radiance
    zenith_color: vec4<f32>,
    // Linear RGB horizon sky radiance
    horizon_color: vec4<f32>,
    // Linear RGB warm dusk/dawn solar-azimuth arch radiance
    dusk_color: vec4<f32>,
    // xyz = linear RGB solar forward-scatter glow, w = Mie glow weight
    sun_glow_color: vec4<f32>,
    // rgb = linear RGB ground horizon color, w = ground opacity in [0.0, 1.0]
    ground_horizon_color: vec4<f32>,
    // rgb = linear RGB ground nadir color, w = unused
    ground_nadir_color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialSkyUniforms;

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
    let atmo_strength = uniforms.sun_dir_and_atmo_strength.w;
    let ground_opacity = uniforms.ground_horizon_color.w;
    if (atmo_strength <= 1e-4 && ground_opacity <= 1e-4) {
        discard;
    }

    let v_cam = unproject_celestial(input.ndc, uniforms.projection);
    let dir_enu = (uniforms.camera_to_topo * vec4<f32>(v_cam, 0.0)).xyz;
    // Evaluate derivatives in uniform control flow before any per-pixel conditional branches.
    let ground_aa = max(fwidth(dir_enu.z), 0.0005);

    var atmo_rgb = vec3<f32>(0.0);
    var atmo_a = 0.0;
    if (atmo_strength > 1e-4) {
        let horizon_mask = smoothstep(-0.05, 0.005, dir_enu.z);
        if (horizon_mask > 1e-4) {
            let up = clamp(dir_enu.z, 0.0, 1.0);
            let h = 1.0 - up;
            let h2 = h * h;
            let h4 = h2 * h2;
            let h8 = h4 * h4;
            let h_mid = pow(max(h, 1e-5), 1.55);
            let horizon_blend = clamp(0.34 * h_mid + 0.66 * h4, 0.0, 1.0);

            let cos_theta = clamp(dot(dir_enu, uniforms.sun_dir_and_atmo_strength.xyz), -1.0, 1.0);
            let rayleigh_phase = 0.86 + 0.18 * cos_theta * cos_theta;

            var sky = mix(uniforms.zenith_color.rgb, uniforms.horizon_color.rgb, horizon_blend) * rayleigh_phase;

            // Warm dusk/dawn horizon arch toward solar azimuth
            let sun_half = 0.5 * (cos_theta + 1.0);
            let sun_wide = sun_half * sun_half;
            let sun_pos = max(cos_theta, 0.0);
            let sun_pos2 = sun_pos * sun_pos;
            let sun_core = sun_pos2 * sun_pos2;
            sky += uniforms.dusk_color.rgb
                * (h8 * (0.35 * sun_wide + 0.85 * sun_core) + h4 * 0.20 * sun_wide);

            // Schlick forward-scattering solar glow (aureole + wide Rayleigh-Mie wrap)
            let inv_mie = 1.0 / max(1.0 - 0.78 * cos_theta, 0.04);
            let mie = (inv_mie * inv_mie) * (0.045 + 0.055 * h4);
            let aureole = pow(max(sun_pos, 1e-5), 12.0) * 0.65;
            sky += uniforms.sun_glow_color.rgb * ((mie + aureole) * uniforms.sun_glow_color.w);

            // Extended Reinhard tonemapping (white point W = 2.35, W^2 = 5.5225) preserves
            // bright daytime sky luminance and circumsolar sky clarity instead of crushing to grey.
            let mapped = clamp(
                (sky * (vec3<f32>(1.0) + sky / 5.5225)) / (vec3<f32>(1.0) + sky),
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
            let lum = dot(mapped, vec3<f32>(0.2126, 0.7152, 0.0722));
            let sun_z = uniforms.sun_dir_and_atmo_strength.z;
            let day_veil = smoothstep(-0.02, 0.035, sun_z);
            let lum_veil = smoothstep(0.002, 0.20, lum);
            let veil = clamp(mix(lum_veil * 0.82, 1.0, day_veil), 0.0, 1.0);
            let scale = atmo_strength * horizon_mask;

            atmo_rgb = mapped * scale;
            atmo_a = veil * scale;
        }
    }

    var ground_rgb = vec3<f32>(0.0);
    var ground_a = 0.0;
    if (ground_opacity > 1e-4) {
        let mask = 1.0 - smoothstep(-ground_aa, ground_aa, dir_enu.z);
        if (mask > 1e-4) {
            let depth = clamp(-dir_enu.z, 0.0, 1.0);
            ground_rgb = mix(uniforms.ground_horizon_color.rgb, uniforms.ground_nadir_color.rgb, depth);
            ground_a = mask * ground_opacity;
        }
    }

    // Porter-Duff: ground OVER atmosphere (output premultiplied alpha).
    let out_rgb = ground_rgb * ground_a + atmo_rgb * (1.0 - ground_a);
    let out_a = ground_a + atmo_a * (1.0 - ground_a);
    if (out_a <= 1e-4 && max(out_rgb.r, max(out_rgb.g, out_rgb.b)) <= 1e-6) {
        discard;
    }

    return vec4<f32>(out_rgb, out_a);
}
