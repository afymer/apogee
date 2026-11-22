#import planetarium::celestial_projection::{
    CelestialProjectionUniforms, project_celestial, unproject_celestial,
}

struct CelestialSolarSystemUniforms {
    camera_to_topo: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    // xy = viewport width, height (physical px); z = layer opacity [0..1]; w = ground_opacity [0..1]
    viewport_and_opacity: vec4<f32>,
    // xyz = unit Sun direction in camera space, w = Sun disc radius (physical px)
    sun_cam_and_disc_px: vec4<f32>,
    // xyz = unit Moon direction in camera space, w = Moon disc radius (physical px)
    moon_cam_and_disc_px: vec4<f32>,
    // xy = 2D unit bright-limb direction on screen, z = sin(phase_angle), w = cos(phase_angle)
    moon_light_params: vec4<f32>,
    // x = sun_quad_scale (quad_px / disc_px), y = moon_quad_scale (quad_px / disc_px), z = sun_alt_deg, w = unused
    render_scales: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialSolarSystemUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) ndc: vec2<f32>,
    @location(2) @interpolate(flat) body_kind: u32,
    @location(3) @interpolate(flat) disc_px: f32,
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let body_kind = input.instance_index;
    let layer_opacity = clamp(uniforms.viewport_and_opacity.z, 0.0, 1.0);

    if (body_kind > 1u || layer_opacity <= 1e-4) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.ndc = vec2<f32>(0.0);
        out.body_kind = body_kind;
        out.disc_px = 0.0;
        return out;
    }

    let is_sun = body_kind == 0u;
    let v_cam = select(uniforms.moon_cam_and_disc_px.xyz, uniforms.sun_cam_and_disc_px.xyz, is_sun);
    let disc_px = select(uniforms.moon_cam_and_disc_px.w, uniforms.sun_cam_and_disc_px.w, is_sun);
    let quad_scale = select(uniforms.render_scales.y, uniforms.render_scales.x, is_sun);
    let quad_radius_px = disc_px * quad_scale;

    let v_len = length(v_cam);
    let v_cam_z = select(0.0, v_cam.z / v_len, v_len > 1e-6);
    let min_z = mix(-0.98, 0.02, uniforms.projection.blend_factor);

    if (v_cam_z <= min_z || disc_px <= 0.1) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.ndc = vec2<f32>(0.0);
        out.body_kind = body_kind;
        out.disc_px = 0.0;
        return out;
    }

    let center_clip = project_celestial(v_cam, uniforms.projection);
    let screen_dim = max(uniforms.viewport_and_opacity.xy, vec2<f32>(1.0, 1.0));
    let corner = input.position.xy;
    let ndc_offset = corner * (quad_radius_px * 2.0 / screen_dim);
    let frag_ndc = center_clip.xy + ndc_offset;

    if (abs(center_clip.x) > 1.45 || abs(center_clip.y) > 1.45) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.ndc = vec2<f32>(0.0);
        out.body_kind = body_kind;
        out.disc_px = 0.0;
        return out;
    }

    out.clip_position = vec4<f32>(frag_ndc.x, frag_ndc.y, center_clip.z, 1.0);
    out.uv = corner;
    out.ndc = frag_ndc;
    out.body_kind = body_kind;
    out.disc_px = disc_px;
    return out;
}

// Procedural multi-scale lunar surface albedo (Maria vs Highlands) on the Moon's unit disc.
fn lunar_surface_albedo(p: vec2<f32>) -> f32 {
    // Major lunar maria basins approximated on the visible near-side hemisphere
    let mare_imbrium = exp(-11.0 * dot(p - vec2<f32>(-0.22, 0.34), p - vec2<f32>(-0.22, 0.34)));
    let mare_serenitatis = exp(-14.0 * dot(p - vec2<f32>(0.18, 0.26), p - vec2<f32>(0.18, 0.26)));
    let mare_tranquillitatis = exp(-12.0 * dot(p - vec2<f32>(0.34, 0.05), p - vec2<f32>(0.34, 0.05)));
    let oceanus_procellarum = exp(-6.5 * dot(p - vec2<f32>(-0.48, 0.04), p - vec2<f32>(-0.48, 0.04)));
    let mare_nubium = exp(-13.0 * dot(p - vec2<f32>(-0.18, -0.32), p - vec2<f32>(-0.18, -0.32)));
    let mare_crisium = exp(-28.0 * dot(p - vec2<f32>(0.64, 0.18), p - vec2<f32>(0.64, 0.18)));

    let maria = clamp(
        0.55 * mare_imbrium
            + 0.48 * mare_serenitatis
            + 0.50 * mare_tranquillitatis
            + 0.45 * oceanus_procellarum
            + 0.38 * mare_nubium
            + 0.46 * mare_crisium,
        0.0,
        0.82
    );

    let fine = 0.04 * sin(p.x * 19.0 + p.y * 11.0) * cos(p.y * 23.0 - p.x * 13.0);
    return clamp(0.92 - 0.34 * maria + fine, 0.48, 1.0);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let r2 = dot(input.uv, input.uv);
    if (r2 > 1.0) {
        discard;
    }

    let v_cam_frag = unproject_celestial(input.ndc, uniforms.projection);
    let dir_enu = (uniforms.camera_to_topo * vec4<f32>(v_cam_frag, 0.0)).xyz;
    let ground_aa = max(fwidth(dir_enu.z), 0.0005);

    let ground_opacity = clamp(uniforms.viewport_and_opacity.w, 0.0, 1.0);
    var ground_vis = 1.0;
    if (ground_opacity > 1e-4) {
        let ground_mask = 1.0 - smoothstep(-ground_aa, ground_aa, dir_enu.z);
        ground_vis = 1.0 - ground_mask * ground_opacity;
    }
    if (ground_vis <= 1e-4) {
        discard;
    }

    let layer_opacity = clamp(uniforms.viewport_and_opacity.z, 0.0, 1.0) * ground_vis;
    let r = sqrt(r2);

    if (input.body_kind == 0u) {
        // Sun: limb-darkened photosphere + warm golden corona
        let quad_scale = max(uniforms.render_scales.x, 1.0);
        let disc_r = r * quad_scale;
        let aa = max(1.15 / max(input.disc_px, 2.0), 0.025);

        let disc_mask = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, disc_r);
        let clamped_dr = min(disc_r, 1.0);
        let mu = sqrt(max(0.0, 1.0 - clamped_dr * clamped_dr));

        // Eddington-style solar limb darkening
        let center_col = vec3<f32>(1.0, 0.98, 0.88);
        let limb_col = vec3<f32>(1.0, 0.56, 0.14);
        let photo_srgb = mix(limb_col, center_col, pow(mu, 0.45));
        let photo_lin = pow(photo_srgb, vec3<f32>(2.2));

        // Corona and solar aureole outside the photosphere
        let halo_dist = max(disc_r - 0.85, 0.0);
        let corona = exp(-2.1 * halo_dist) * pow(max(0.0, 1.0 - r), 1.8) * 0.72;
        let corona_lin = pow(vec3<f32>(1.0, 0.78, 0.34), vec3<f32>(2.2));

        let color = mix(corona_lin, photo_lin, disc_mask);
        let alpha = clamp(max(disc_mask, corona) * layer_opacity, 0.0, 1.0);
        if (alpha <= 1e-4) {
            discard;
        }
        return vec4<f32>(color, alpha);
    } else {
        // Moon: 3D spherical phase lighting + procedural Maria albedo + earthshine + soft halo
        let quad_scale = max(uniforms.render_scales.y, 1.0);
        let disc_xy = input.uv * quad_scale;
        let disc_r = r * quad_scale;
        let aa = max(1.15 / max(input.disc_px, 2.0), 0.025);

        let disc_mask = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, disc_r);
        let clamped_xy = disc_xy / max(disc_r, 1.0);
        let p_xy = select(disc_xy, clamped_xy, disc_r > 1.0);
        let pz = sqrt(max(0.0, 1.0 - min(dot(p_xy, p_xy), 1.0)));
        let n_local = vec3<f32>(p_xy.x, p_xy.y, pz);

        let limb_dir = uniforms.moon_light_params.xy;
        let sin_alpha = uniforms.moon_light_params.z;
        let cos_alpha = uniforms.moon_light_params.w;
        let light_dir = normalize(vec3<f32>(limb_dir.x * sin_alpha, limb_dir.y * sin_alpha, cos_alpha));

        let ndotl = dot(n_local, light_dir);
        let term_aa = max(0.06, 1.6 / max(input.disc_px, 4.0));
        let lit_mask = smoothstep(-term_aa * 0.4, term_aa * 0.85, ndotl);

        // Lommel-Seeliger + Lambertian lunar reflectance law (avoids artificial limb darkening on Full Moon)
        let cos_i = max(ndotl, 0.0);
        let cos_e = max(pz, 0.05);
        let lommel = cos_i / max(cos_i + cos_e, 0.05);
        let lambert = pow(cos_i, 0.75);
        let brdf = mix(lommel * 1.45, lambert, 0.38);

        let albedo = lunar_surface_albedo(p_xy);
        let sunlit_srgb = vec3<f32>(0.97, 0.95, 0.90) * albedo * clamp(brdf, 0.15, 1.15);
        let earthshine_srgb = vec3<f32>(0.11, 0.14, 0.20) * albedo;

        let surface_srgb = mix(earthshine_srgb, sunlit_srgb, lit_mask);
        let surface_lin = pow(max(surface_srgb, vec3<f32>(0.0)), vec3<f32>(2.2));

        // Daytime sky contrast adaptation: unlit lunar side blends softly during bright daylight
        let sun_alt = uniforms.render_scales.z;
        let day_factor = smoothstep(-4.0, 8.0, sun_alt);
        let night_side_alpha = mix(0.88, 0.22, day_factor);
        let disc_alpha = disc_mask * mix(night_side_alpha, 0.98, lit_mask);

        // Subtle outer lunar glow proportional to illuminated fraction
        let illum_frac = clamp(0.5 * (1.0 + cos_alpha), 0.0, 1.0);
        let halo_dist = max(disc_r - 0.9, 0.0);
        let halo = exp(-3.2 * halo_dist) * pow(max(0.0, 1.0 - r), 2.0) * (0.12 + 0.26 * illum_frac) * (1.0 - 0.65 * day_factor);
        let halo_lin = pow(vec3<f32>(0.82, 0.88, 0.98), vec3<f32>(2.2));

        let color = mix(halo_lin, surface_lin, disc_mask);
        let alpha = clamp(max(disc_alpha, halo) * layer_opacity, 0.0, 1.0);
        if (alpha <= 1e-4) {
            discard;
        }
        return vec4<f32>(color, alpha);
    }
}
