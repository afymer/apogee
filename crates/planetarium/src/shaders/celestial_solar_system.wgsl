#import planetarium::celestial_projection::{
    CelestialProjectionUniforms, project_celestial, unproject_celestial,
}

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;
const POINT_BODY_COUNT: u32 = 11u;

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
    // x = sun_quad_scale, y = moon_quad_scale, z = sun_alt_deg, w = has_moon_textures (0 or 1)
    render_scales: vec4<f32>,
    // x = atmosphere_strength [0..1], y = moon_alt_deg, zw = 2D unit lunar-North direction on Moon disc
    atmo_and_moon_north: vec4<f32>,
    // x = optical libration lon (rad), y = optical libration lat (rad), z = has_planet_textures (0 or 1), w = unused
    moon_libration: vec4<f32>,
    // Point-like bodies (planets, satellites): xyz = unit camera-space direction, w = disc radius (physical px)
    point_cam_and_disc_px: array<vec4<f32>, 11>,
    // rgb = sRGB tint, w = glow strength [0..1]
    point_tint_and_strength: array<vec4<f32>, 11>,
    // xy = 2D unit bright-limb direction, z = sin(phase_angle), w = cos(phase_angle)
    point_light: array<vec4<f32>, 11>,
    // xy = 2D unit screen direction of the north rotation pole, z = sub-observer lon (rad, east +), w = sub-observer lat (rad)
    point_axis: array<vec4<f32>, 11>,
    // x = ring inner radius, y = ring outer radius (planet radii; 0 = no rings), z = ring profile layer
    point_ring: array<vec4<f32>, 11>,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialSolarSystemUniforms;
@group(0) @binding(1) var moon_albedo_texture: texture_2d<f32>;
@group(0) @binding(2) var moon_normal_texture: texture_2d<f32>;
@group(0) @binding(3) var moon_sampler: sampler;
@group(0) @binding(4) var planet_albedo_texture: texture_2d_array<f32>;
@group(0) @binding(5) var ring_texture: texture_2d_array<f32>;

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
    @location(4) @interpolate(flat) quad_px: f32,
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let body_kind = input.instance_index;
    let layer_opacity = clamp(uniforms.viewport_and_opacity.z, 0.0, 1.0);

    out.quad_px = 1.0;
    if (body_kind > 1u + POINT_BODY_COUNT || layer_opacity <= 1e-4) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.ndc = vec2<f32>(0.0);
        out.body_kind = body_kind;
        out.disc_px = 0.0;
        return out;
    }

    let is_sun = body_kind == 0u;
    var v_cam = select(uniforms.moon_cam_and_disc_px.xyz, uniforms.sun_cam_and_disc_px.xyz, is_sun);
    var disc_px = select(uniforms.moon_cam_and_disc_px.w, uniforms.sun_cam_and_disc_px.w, is_sun);
    let quad_scale = select(uniforms.render_scales.y, uniforms.render_scales.x, is_sun);
    var quad_radius_px = disc_px * quad_scale;
    if (body_kind >= 2u) {
        let slot = body_kind - 2u;
        let point_body = uniforms.point_cam_and_disc_px[slot];
        v_cam = point_body.xyz;
        disc_px = point_body.w;
        let strength = uniforms.point_tint_and_strength[slot].w;
        // Room for the glow: grows with brightness, never smaller than the disc itself.
        quad_radius_px = disc_px * 1.6 + 5.0 + 22.0 * strength;
        // Rings extend well beyond the disc
        quad_radius_px = max(quad_radius_px, disc_px * uniforms.point_ring[slot].y * 1.04);
    }
    out.quad_px = quad_radius_px;

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
    let ndc_half_ext = quad_radius_px * 2.0 / screen_dim;
    let ndc_offset = corner * ndc_half_ext;
    let frag_ndc = center_clip.xy + ndc_offset;

    if (abs(center_clip.x) - ndc_half_ext.x > 1.25 || abs(center_clip.y) - ndc_half_ext.y > 1.25) {
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

const PLANET_TEXTURED_COUNT: u32 = 7u;

// Radial ring profile (rgb + alpha) at planet-radii distance `rho`, or transparent outside the ring.
fn ring_profile(ring: vec4<f32>, rho: f32, disc_px: f32, sin_lat: f32) -> vec4<f32> {
    let t = (rho - ring.x) / max(ring.y - ring.x, 1e-4);
    if (t <= 0.0 || t >= 1.0) {
        return vec4<f32>(0.0);
    }
    // Radial texels covered by one screen pixel (the ring is foreshortened by sin(lat))
    let footprint = 1.0 / (max(disc_px, 1.0) * max(abs(sin_lat), 0.15));
    let lod = max(log2(2048.0 * footprint / max(ring.y - ring.x, 1e-4)), 0.0);
    return textureSampleLevel(ring_texture, moon_sampler, vec2<f32>(t, 0.5), i32(ring.z), lod);
}

// Planets and satellites: a textured, phase-shaded disc with optional rings, inside a brightness-scaled glow.
fn point_body_color(input: VertexOutput, r: f32, layer_opacity: f32, atmo_strength: f32, sun_alt: f32) -> vec4<f32> {
    let slot = input.body_kind - 2u;
    let tint_s = uniforms.point_tint_and_strength[slot];
    let light = uniforms.point_light[slot];
    let axis = uniforms.point_axis[slot];
    let ring = uniforms.point_ring[slot];
    let center_cam = uniforms.point_cam_and_disc_px[slot].xyz;
    let disc_px = max(input.disc_px, 0.5);
    let d_px = r * input.quad_px;
    let strength = tint_s.w;

    // Atmospheric extinction and reddening toward the horizon
    let enu = (uniforms.camera_to_topo * vec4<f32>(center_cam, 0.0)).xyz;
    let mu = max(enu.z / max(length(enu), 1e-6), 0.0);
    let airmass = 1.0 / (mu + 0.055);
    let k = max(airmass - 1.0, 0.0) * atmo_strength;
    let trans = exp(-vec3<f32>(0.06, 0.14, 0.34) * k * 0.28);
    let dim = exp(-0.06 * k);

    // Sunlit planets are lost in the daytime sky unless the sky layer is disabled
    let day_veil = atmo_strength * smoothstep(-5.0, 6.0, sun_alt);
    let visibility = 1.0 - 0.97 * day_veil;

    let tint_lin = pow(tint_s.rgb, vec3<f32>(2.2)) * trans;
    let edge_fade = pow(max(1e-5, 1.0 - r), 2.0);

    // Frame: x = planet east (screen right when north is up), y = planet north, z = toward observer
    let raw_north = axis.xy;
    let north_len = length(raw_north);
    let north_dir = select(vec2<f32>(0.0, 1.0), raw_north / north_len, north_len > 1e-4);
    let east_dir = vec2<f32>(north_dir.y, -north_dir.x);
    let p_screen = input.uv * input.quad_px / disc_px;
    let q = vec2<f32>(dot(p_screen, east_dir), dot(p_screen, north_dir));
    let q2 = dot(q, q);
    let inside = q2 <= 1.0;
    let pz = sqrt(max(0.0, 1.0 - min(q2, 1.0)));
    let nm = vec3<f32>(q, pz);

    let light_screen = normalize(vec3<f32>(light.xy * light.z, light.w));
    let lm = normalize(vec3<f32>(dot(light_screen.xy, east_dir), dot(light_screen.xy, north_dir), light_screen.z));

    let lon_c = axis.z;
    let lat_c = axis.w;
    let sin_lat = sin(lat_c);
    let cos_lat = cos(lat_c);
    let pole_axis = vec3<f32>(0.0, cos_lat, sin_lat);

    // Ring shadow on the planet: trace from the surface toward the Sun and test the ring plane
    var ring_shadow = 0.0;
    let has_ring = ring.y > 0.0 && uniforms.moon_libration.z > 0.5;
    let ring_scale = smoothstep(1.5, 4.0, disc_px);
    if (has_ring && inside) {
        let l_dot_a = dot(lm, pole_axis);
        if (abs(l_dot_a) > 1e-3) {
            let t_hit = -dot(nm, pole_axis) / l_dot_a;
            if (t_hit > 0.0) {
                let hit = nm + lm * t_hit;
                ring_shadow = ring_profile(ring, length(hit), disc_px, sin_lat).a * ring_scale;
            }
        }
    }

    // Disc: phase shading, limb darkening, and the surface texture when available
    let aa = 0.75;
    let disc_mask = 1.0 - smoothstep(max(disc_px - aa, 0.0), disc_px + aa, d_px);
    let term_aa = max(0.05, 1.2 / disc_px);
    let ndotl = dot(nm, lm);
    let lit = smoothstep(-term_aa, term_aa, ndotl);
    let lambert = mix(0.0, pow(clamp(ndotl, 0.0, 1.0), 0.75), lit);
    var albedo = tint_lin;
    let resolved = smoothstep(2.5, 7.0, disc_px);
    if (uniforms.moon_libration.z > 0.5 && slot < PLANET_TEXTURED_COUNT && inside) {
        let sin_b = sin(lat_c);
        let cos_b = cos(lat_c);
        let sin_l = sin(lon_c);
        let cos_l = cos(lon_c);
        let n_b = vec3<f32>(nm.x, nm.y * cos_b + nm.z * sin_b, -nm.y * sin_b + nm.z * cos_b);
        let n_sel = normalize(vec3<f32>(n_b.x * cos_l + n_b.z * sin_l, n_b.y, -n_b.x * sin_l + n_b.z * cos_l));
        let lon = select(0.0, atan2(n_sel.x, n_sel.z), abs(n_sel.x) + abs(n_sel.z) > 1e-5);
        let lat = asin(clamp(n_sel.y, -1.0, 1.0));
        let tex_uv = vec2<f32>(fract(0.5 + lon / TAU), clamp(0.5 - lat / PI, 0.001, 0.999));
        // Texels covered by one screen pixel at the disc center (2048 texels around 2*pi radians)
        let lod = max(log2(326.0 / disc_px) + 0.4, 0.0);
        let tex = textureSampleLevel(planet_albedo_texture, moon_sampler, tex_uv, i32(slot), lod).rgb;
        albedo = mix(tint_lin, tex * trans, resolved);
    }
    let limb = 0.62 + 0.38 * sqrt(pz);
    let shade = (0.03 + 0.97 * lambert) * limb * (1.0 - 0.85 * ring_shadow);
    // Tiny discs are effectively unresolved: keep them fully bright
    let disc_shade = mix(1.0, shade, resolved);
    let disc_rgb = albedo * (0.85 + 0.6 * strength) * 1.55 * disc_shade;

    // Glow: tight core plus a broader halo, both scaled by apparent brightness
    let glow_core = exp(-d_px / (1.2 + 2.6 * strength));
    let glow_halo = exp(-d_px / (4.0 + 10.0 * strength)) * 0.22;
    let glow = (glow_core * 0.55 + glow_halo) * strength * strength * edge_fade * dim;
    let glow_rgb = mix(tint_lin, vec3<f32>(1.0), 0.35) * glow;

    // Rings: intersect the pixel ray with the ring plane (normal = pole axis)
    var ring_rgb = vec3<f32>(0.0);
    var ring_a = 0.0;
    var ring_front = false;
    if (has_ring) {
        let s = select(-max(abs(sin_lat), 0.004), max(abs(sin_lat), 0.004), sin_lat >= 0.0);
        let depth = -q.y / s;
        let rho = length(vec2<f32>(q.x, depth));
        let profile = ring_profile(ring, rho, disc_px, sin_lat);
        let ring_z = depth * cos_lat;
        let p_ring = vec3<f32>(q.x, q.y, ring_z);
        let pl = dot(p_ring, lm);
        // Planet shadow cast onto the ring
        let miss = sqrt(max(dot(p_ring, p_ring) - pl * pl, 0.0));
        let shadow = select(0.0, 1.0 - smoothstep(0.97, 1.03, miss), pl < 0.0);
        // Lit face is the one toward the Sun; the far face only shows faint transmitted light
        let lit_face = select(0.10, 1.0, dot(lm, pole_axis) * sin_lat > 0.0);
        let thin = smoothstep(0.004, 0.03, abs(sin_lat));
        ring_a = profile.a * ring_scale * thin;
        ring_rgb = profile.rgb * trans * 1.8 * lit_face * (1.0 - 0.92 * shadow);
        ring_front = !inside || ring_z > pz;
    }

    let planet_a = disc_mask;
    let planet_pm = disc_rgb * disc_mask * dim;
    let ring_pm = ring_rgb * ring_a * dim;
    var rgb: vec3<f32>;
    var alpha: f32;
    if (ring_front) {
        rgb = ring_pm + planet_pm * (1.0 - ring_a);
        alpha = ring_a + planet_a * (1.0 - ring_a);
    } else {
        rgb = planet_pm + ring_pm * (1.0 - planet_a);
        alpha = planet_a + ring_a * (1.0 - planet_a);
    }
    rgb += glow_rgb * (1.0 - planet_a) * (1.0 - ring_a);
    rgb = rgb * visibility * layer_opacity;
    alpha = clamp(alpha * visibility * layer_opacity * 0.95, 0.0, 1.0);
    if (alpha <= 1e-4 && max(rgb.r, max(rgb.g, rgb.b)) <= 1e-5) {
        discard;
    }
    return vec4<f32>(rgb, alpha);
}


fn hash21(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(123.34, 456.21));
    let r = q + dot(q, q + 45.32);
    return fract(r.x * r.y);
}

fn value_noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn lunar_fbm(p_in: vec2<f32>) -> f32 {
    let rot = mat2x2<f32>(0.80, -0.60, 0.60, 0.80);
    var p = p_in;
    var sum = 0.0;
    var amp = 0.5;
    for (var i = 0; i < 4; i = i + 1) {
        sum += amp * value_noise2(p);
        p = rot * p * 2.05 + vec2<f32>(17.1, 31.7);
        amp *= 0.5;
    }
    return sum;
}

// Fallback procedural albedo when external lunar textures are not loaded
fn procedural_lunar_albedo(moon_uv: vec2<f32>) -> f32 {
    let warp = vec2<f32>(
        lunar_fbm(moon_uv * 3.2 + vec2<f32>(1.7, 9.2)),
        lunar_fbm(moon_uv * 3.2 + vec2<f32>(8.3, 2.8)),
    ) - vec2<f32>(0.5);
    let wp = moon_uv + warp * 0.14;

    let mare_imbrium = exp(-10.5 * dot(wp - vec2<f32>(-0.22, 0.34), wp - vec2<f32>(-0.22, 0.34)));
    let mare_serenitatis = exp(-14.5 * dot(wp - vec2<f32>(0.18, 0.25), wp - vec2<f32>(0.18, 0.25)));
    let mare_tranquillitatis = exp(-11.5 * dot(wp - vec2<f32>(0.34, 0.05), wp - vec2<f32>(0.34, 0.05)));
    let mare_fecunditatis = exp(-18.0 * dot(wp - vec2<f32>(0.54, -0.14), wp - vec2<f32>(0.54, -0.14)));
    let mare_nectaris = exp(-24.0 * dot(wp - vec2<f32>(0.36, -0.26), wp - vec2<f32>(0.36, -0.26)));
    let oceanus_procellarum = exp(-6.2 * dot(wp - vec2<f32>(-0.48, 0.04), wp - vec2<f32>(-0.48, 0.04)));
    let mare_nubium = exp(-13.5 * dot(wp - vec2<f32>(-0.18, -0.32), wp - vec2<f32>(-0.18, -0.32)));
    let mare_humorum = exp(-26.0 * dot(wp - vec2<f32>(-0.46, -0.34), wp - vec2<f32>(-0.46, -0.34)));
    let mare_crisium = exp(-28.0 * dot(wp - vec2<f32>(0.64, 0.19), wp - vec2<f32>(0.64, 0.19)));

    let maria = clamp(
        0.56 * mare_imbrium
            + 0.48 * mare_serenitatis
            + 0.50 * mare_tranquillitatis
            + 0.42 * mare_fecunditatis
            + 0.36 * mare_nectaris
            + 0.46 * oceanus_procellarum
            + 0.38 * mare_nubium
            + 0.34 * mare_humorum
            + 0.46 * mare_crisium,
        0.0,
        0.84,
    );

    let tycho = exp(-36.0 * dot(wp - vec2<f32>(-0.14, -0.68), wp - vec2<f32>(-0.14, -0.68)));
    let copernicus = exp(-30.0 * dot(wp - vec2<f32>(-0.34, 0.14), wp - vec2<f32>(-0.34, 0.14)));
    let ejecta = 0.09 * tycho + 0.06 * copernicus;

    let regolith_mid = lunar_fbm(moon_uv * 9.5 + vec2<f32>(4.1, 7.3)) - 0.5;
    let regolith_fine = lunar_fbm(moon_uv * 24.0 + vec2<f32>(11.2, 3.6)) - 0.5;
    let detail = 0.09 * regolith_mid + 0.045 * regolith_fine;

    return clamp(0.92 - 0.36 * maria + ejecta + detail, 0.44, 1.0);
}

fn sample_moon_albedo(uv: vec2<f32>, disc_px: f32) -> vec3<f32> {
    let base = textureSampleLevel(moon_albedo_texture, moon_sampler, uv, 0.0).rgb;
    let blur_blend = smoothstep(42.0, 12.0, disc_px);
    if (blur_blend <= 1e-3) {
        return base;
    }
    let d = clamp(0.24 / max(disc_px, 4.0), 0.0, 0.012);
    let t0 = textureSampleLevel(moon_albedo_texture, moon_sampler, vec2<f32>(fract(uv.x + d), clamp(uv.y + d * 0.5, 0.001, 0.999)), 0.0).rgb;
    let t1 = textureSampleLevel(moon_albedo_texture, moon_sampler, vec2<f32>(fract(uv.x - d), clamp(uv.y - d * 0.5, 0.001, 0.999)), 0.0).rgb;
    let t2 = textureSampleLevel(moon_albedo_texture, moon_sampler, vec2<f32>(fract(uv.x - d * 0.5), clamp(uv.y + d, 0.001, 0.999)), 0.0).rgb;
    let t3 = textureSampleLevel(moon_albedo_texture, moon_sampler, vec2<f32>(fract(uv.x + d * 0.5), clamp(uv.y - d, 0.001, 0.999)), 0.0).rgb;
    return mix(base, 0.25 * (t0 + t1 + t2 + t3), blur_blend * 0.8);
}

fn sample_moon_normal_height(uv: vec2<f32>, disc_px: f32) -> vec4<f32> {
    let base = textureSampleLevel(moon_normal_texture, moon_sampler, uv, 0.0);
    let blur_blend = smoothstep(42.0, 12.0, disc_px);
    if (blur_blend <= 1e-3) {
        return base;
    }
    let d = clamp(0.22 / max(disc_px, 4.0), 0.0, 0.010);
    let t0 = textureSampleLevel(moon_normal_texture, moon_sampler, vec2<f32>(fract(uv.x + d), clamp(uv.y + d * 0.5, 0.001, 0.999)), 0.0);
    let t1 = textureSampleLevel(moon_normal_texture, moon_sampler, vec2<f32>(fract(uv.x - d), clamp(uv.y - d * 0.5, 0.001, 0.999)), 0.0);
    let t2 = textureSampleLevel(moon_normal_texture, moon_sampler, vec2<f32>(fract(uv.x - d * 0.5), clamp(uv.y + d, 0.001, 0.999)), 0.0);
    let t3 = textureSampleLevel(moon_normal_texture, moon_sampler, vec2<f32>(fract(uv.x + d * 0.5), clamp(uv.y - d, 0.001, 0.999)), 0.0);
    return mix(base, 0.25 * (t0 + t1 + t2 + t3), blur_blend * 0.75);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let v_cam_frag = unproject_celestial(input.ndc, uniforms.projection);
    let dir_enu = (uniforms.camera_to_topo * vec4<f32>(v_cam_frag, 0.0)).xyz;
    // Evaluate derivatives in uniform control flow before per-pixel discards or branches.
    let ground_aa = max(fwidth(dir_enu.z), 0.0005);

    let r2 = dot(input.uv, input.uv);
    if (r2 > 1.0) {
        discard;
    }

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
    let atmo_strength = clamp(uniforms.atmo_and_moon_north.x, 0.0, 1.0);
    let sun_alt = uniforms.render_scales.z;
    let r = sqrt(r2);

    if (input.body_kind >= 2u) {
        return point_body_color(input, r, layer_opacity, atmo_strength, sun_alt);
    }

    if (input.body_kind == 0u) {
        // Sun: atmospheric beam transmittance, zoom-adaptive photosphere, and luminous aureole
        let quad_scale = max(uniforms.render_scales.x, 1.0);
        let disc_r = r * quad_scale;
        let aa = clamp(1.25 / max(input.disc_px, 2.0), 0.00005, 0.15);
        let disc_mask = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, disc_r);

        let clamped_dr = min(disc_r, 1.0);
        let mu = sqrt(max(0.0, 1.0 - clamped_dr * clamped_dr));

        let sun_mu = max(sin(radians(max(sun_alt, 0.0))), 0.0);
        let sun_airmass = 1.0 / (sun_mu + 0.055);
        let sun_k = max(sun_airmass - 1.0, 0.0) * 0.22 * atmo_strength;
        let sun_trans = exp(-vec3<f32>(0.058, 0.135, 0.331) * sun_k);

        // Reveal solar limb darkening and granulation cleanly at high telescopic magnification
        let zoom_detail = 1.0 - smoothstep(0.015, 0.09, uniforms.projection.fov_y);
        let core_col = mix(sun_trans, vec3<f32>(1.0), 0.90);
        let limb_tint = sun_trans * vec3<f32>(1.0, 0.78, 0.45);
        let limb_col = mix(limb_tint, core_col, 0.42 * (1.0 - zoom_detail));
        let safe_mu = max(mu, 1e-5);
        let photo_col = mix(limb_col, core_col, pow(safe_mu, mix(0.30, 0.58, zoom_detail)));

        let granulation = (lunar_fbm(input.uv * quad_scale * 22.0) - 0.5) * 0.045;
        let eddington = mix(0.72, 1.08, pow(safe_mu, 0.52));
        let wide_profile = mix(0.95, 1.15, pow(safe_mu, 0.35));
        let photo_lin = photo_col * (mix(wide_profile, eddington, zoom_detail) + granulation * zoom_detail);

        // Multi-layer solar aureole, chromospheric bloom, and ciliary glare rays
        let edge_fade = pow(max(1e-5, 1.0 - r), 2.3);
        let halo_dist = max(disc_r - 0.96, 0.0);
        let inner_bloom = exp(-3.4 * halo_dist) * 0.92 * edge_fade;
        let aureole = (1.0 / pow(1.0 + 1.15 * max(disc_r - 0.88, 0.0), 2.15)) * 0.42 * edge_fade;

        let ang = select(0.0, atan2(input.uv.y, input.uv.x), r > 1e-5);
        let ray_pattern = 0.5
            + 0.5
                * (0.55 * cos(ang * 6.0 + 0.4)
                    + 0.30 * cos(ang * 10.0 - 1.1)
                    + 0.15 * cos(ang * 14.0 + 2.2));
        let rays = exp(-1.6 * max(disc_r - 0.95, 0.0))
            * ray_pattern
            * 0.22
            * edge_fade
            * (1.0 - 0.8 * zoom_detail);

        let low_sun_glare = mix(0.72, 1.0, smoothstep(-2.0, 15.0, sun_alt));
        let glare = (inner_bloom + aureole * (1.0 - 0.68 * zoom_detail) + rays) * low_sun_glare;
        let glare_col = mix(
            sun_trans * vec3<f32>(1.0, 0.88, 0.66),
            vec3<f32>(1.0, 0.98, 0.94),
            clamp(inner_bloom * 0.65, 0.0, 1.0),
        );

        let premul_rgb =
            (photo_lin * disc_mask + glare_col * glare * (1.0 - 0.75 * disc_mask)) * layer_opacity;
        let alpha = clamp(disc_mask * layer_opacity, 0.0, 1.0);
        if (alpha <= 1e-4 && max(premul_rgb.r, max(premul_rgb.g, premul_rgb.b)) <= 1e-5) {
            discard;
        }
        return vec4<f32>(premul_rgb, alpha);
    } else {
        // Moon: real NASA LROC/LOLA textures, optical libration, normal-mapped relief,
        // grazing-sunlight crater self-shadowing, and daytime atmospheric blending
        let quad_scale = max(uniforms.render_scales.y, 1.0);
        let disc_xy = input.uv * quad_scale;
        let disc_r = r * quad_scale;
        let aa = clamp(1.25 / max(input.disc_px, 2.0), 0.00005, 0.15);
        let disc_mask = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, disc_r);

        let clamped_xy = disc_xy / max(disc_r, 1.0);
        let p_xy = select(disc_xy, clamped_xy, disc_r > 1.0);
        let pz = sqrt(max(0.0, 1.0 - min(dot(p_xy, p_xy), 1.0)));

        let raw_north = uniforms.atmo_and_moon_north.zw;
        let north_len = length(raw_north);
        let north_dir = select(vec2<f32>(0.0, 1.0), raw_north / north_len, north_len > 1e-4);
        let east_dir = vec2<f32>(north_dir.y, -north_dir.x);

        let moon_uv = vec2<f32>(dot(p_xy, east_dir), dot(p_xy, north_dir));
        let nm = vec3<f32>(moon_uv.x, moon_uv.y, pz);

        let limb_dir = uniforms.moon_light_params.xy;
        let sin_alpha = uniforms.moon_light_params.z;
        let cos_alpha = uniforms.moon_light_params.w;
        let light_screen = normalize(vec3<f32>(limb_dir.x * sin_alpha, limb_dir.y * sin_alpha, cos_alpha));
        let lm = normalize(vec3<f32>(dot(light_screen.xy, east_dir), dot(light_screen.xy, north_dir), light_screen.z));

        // Rotate view-aligned sphere normal and light vector by optical libration (l', b') into selenographic frame
        let sin_b = sin(uniforms.moon_libration.y);
        let cos_b = cos(uniforms.moon_libration.y);
        let sin_l = sin(uniforms.moon_libration.x);
        let cos_l = cos(uniforms.moon_libration.x);

        let n_b = vec3<f32>(nm.x, nm.y * cos_b + nm.z * sin_b, -nm.y * sin_b + nm.z * cos_b);
        let n_sel = normalize(vec3<f32>(n_b.x * cos_l + n_b.z * sin_l, n_b.y, -n_b.x * sin_l + n_b.z * cos_l));

        let l_b = vec3<f32>(lm.x, lm.y * cos_b + lm.z * sin_b, -lm.y * sin_b + lm.z * cos_b);
        let l_sel = normalize(vec3<f32>(l_b.x * cos_l + l_b.z * sin_l, l_b.y, -l_b.x * sin_l + l_b.z * cos_l));

        // Selenographic spherical coordinates & equirectangular UV
        let lon = select(0.0, atan2(n_sel.x, n_sel.z), abs(n_sel.x) + abs(n_sel.z) > 1e-5);
        let lat = asin(clamp(n_sel.y, -1.0, 1.0));
        let tex_uv = vec2<f32>(
            fract(0.5 + lon / TAU),
            clamp(0.5 - lat / PI, 0.001, 0.999),
        );

        let sin_lon = sin(lon);
        let cos_lon = cos(lon);
        let sin_lat = sin(lat);
        let cos_lat = cos(lat);
        let t_east = vec3<f32>(cos_lon, 0.0, -sin_lon);
        let t_north = vec3<f32>(-sin_lat * sin_lon, cos_lat, -sin_lat * cos_lon);

        let cos_i0 = dot(n_sel, l_sel);
        let full_weight = clamp(0.5 * (1.0 + cos_alpha), 0.0, 1.0);
        let has_tex = uniforms.render_scales.w > 0.5;
        let limb_taper = smoothstep(0.0, 0.14, pz);

        var albedo_lin = vec3<f32>(1.0);
        var ndotl_pert = cos_i0;
        var rim_shadow = 1.0;

        if (has_tex) {
            albedo_lin = sample_moon_albedo(tex_uv, input.disc_px) * 2.35;
            let nh = sample_moon_normal_height(tex_uv, input.disc_px);
            let t_norm = normalize(nh.rgb * 2.0 - vec3<f32>(1.0));
            let h0 = nh.a;

            // Amplify crater relief under low sunlight near the terminator; flatten under Full Moon opposition
            let low_sun = 1.0 - smoothstep(0.08, 0.72, max(cos_i0, 0.0));
            let full_fade = 1.0 - 0.78 * full_weight * full_weight;
            let zoom_boost = mix(0.85, 1.35, smoothstep(20.0, 140.0, input.disc_px));
            let relief_scale = (0.45 + 1.05 * low_sun) * full_fade * zoom_boost * limb_taper;

            let n_pert = normalize(
                n_sel * max(t_norm.z, 0.05) + (t_east * t_norm.x + t_north * t_norm.y) * relief_scale,
            );
            ndotl_pert = dot(n_pert, l_sel);

            // Height ray-marched crater rim self-shadowing under grazing sunlight
            let l_east = dot(l_sel, t_east);
            let l_north = dot(l_sel, t_north);
            let horiz_len = length(vec2<f32>(l_east, l_north));
            var shadow_occ = 0.0;
            if (cos_i0 > -0.15 && cos_i0 < 0.55 && horiz_len > 1e-4) {
                let dir_uv = vec2<f32>(
                    (l_east / horiz_len) / (TAU * max(cos_lat, 0.12)),
                    -(l_north / horiz_len) / PI,
                );
                let sun_tan = max(cos_i0, 0.0) / horiz_len;
                let step_arc = 0.0085;
                let height_scale = 0.045;
                for (var k = 1; k <= 3; k = k + 1) {
                    let arc = step_arc * f32(k);
                    let s_uv = tex_uv + dir_uv * arc;
                    let h_sample = textureSampleLevel(
                        moon_normal_texture,
                        moon_sampler,
                        vec2<f32>(fract(s_uv.x), clamp(s_uv.y, 0.001, 0.999)),
                        0.0,
                    ).a;
                    let excess = (h_sample - h0) * height_scale - sun_tan * arc;
                    shadow_occ = max(shadow_occ, smoothstep(0.0, 0.0022, excess));
                }
                shadow_occ *= 1.0 - smoothstep(0.32, 0.55, cos_i0);
            }
            rim_shadow = 1.0 - 0.88 * shadow_occ * full_fade * limb_taper;

            // Subtle micro-regolith grain at deep telescopic magnification (>300 px radius)
            let micro_weight = smoothstep(300.0, 1100.0, input.disc_px);
            if (micro_weight > 1e-3) {
                let micro_grain = lunar_fbm(tex_uv * vec2<f32>(3200.0, 1600.0)) - 0.5;
                albedo_lin *= 1.0 + 0.12 * micro_grain * micro_weight;
                ndotl_pert += 0.08 * micro_grain * micro_weight * low_sun * full_fade * limb_taper;
            }
        } else {
            let proc_a = procedural_lunar_albedo(moon_uv);
            albedo_lin = pow(vec3<f32>(0.98, 0.96, 0.92) * proc_a, vec3<f32>(2.2));
            let relief = (lunar_fbm(moon_uv * 12.0) - 0.5) * 0.07 * pz * limb_taper;
            ndotl_pert = cos_i0 + relief;
        }

        let horizon_gate = smoothstep(-0.16 * limb_taper - 0.02, -0.005, cos_i0);
        let term_aa = max(0.025, 1.15 / max(input.disc_px, 4.0));
        let lit_mask = smoothstep(-term_aa * 0.5, term_aa * 0.5, ndotl_pert) * horizon_gate;

        // Lunar-Lambert reflectance law: Lommel-Seeliger backscatter near Full Moon, sharper slope contrast at quarters
        let cos_i = max(ndotl_pert, 0.0);
        let cos_e = max(pz, 0.06);
        let lommel = cos_i / max(cos_i + cos_e, 1e-3);
        let brdf = mix(cos_i, lommel * 1.38, mix(0.54, 0.86, full_weight));
        let lit_brdf = clamp(brdf * lit_mask * rim_shadow, 0.0, 1.45);

        // Atmospheric extinction when Moon is low near the horizon
        let moon_alt = uniforms.atmo_and_moon_north.y;
        let moon_mu = max(sin(radians(max(moon_alt, 0.0))), 0.0);
        let moon_airmass = 1.0 / (moon_mu + 0.055);
        let moon_k = max(moon_airmass - 1.0, 0.0) * 0.14 * atmo_strength;
        let moon_trans = exp(-vec3<f32>(0.06, 0.14, 0.34) * moon_k);

        let day_sky_veil = atmo_strength * smoothstep(-5.0, 6.0, sun_alt);

        // In daytime, atmospheric sky light sits in front of the Moon: unlit half disappears cleanly
        // into the sky with zero dark circle or outline ring, while sunlit maria stay sky-tinted.
        let sunlit_lin =
            albedo_lin * lit_brdf * moon_trans * mix(1.0, 0.42, day_sky_veil);

        let earthshine_strength = (1.0 - day_sky_veil) * (1.0 - 0.65 * full_weight);
        let earthshine_lin =
            albedo_lin * vec3<f32>(0.008, 0.013, 0.024) * earthshine_strength * moon_trans;

        let disc_lin = sunlit_lin + earthshine_lin * (1.0 - lit_mask);

        let albedo_luma = dot(albedo_lin, vec3<f32>(0.2126, 0.7152, 0.0722));
        let day_lit_occlude = clamp(lit_brdf * min(albedo_luma, 1.0) * 0.20, 0.0, 0.20);
        let disc_occlude = disc_mask * mix(0.98, day_lit_occlude, day_sky_veil);

        // Subtle outer lunar glow strictly outside the disc, weighted toward the sunlit limb at night
        let sun_side_weight = smoothstep(-0.3, 0.5, cos_i0);
        let halo_dist = max(disc_r - 0.92, 0.0);
        let halo = exp(-3.4 * halo_dist)
            * pow(max(1e-5, 1.0 - r), 2.0)
            * (0.08 + 0.22 * full_weight)
            * (1.0 - day_sky_veil)
            * sun_side_weight;
        let halo_lin = pow(vec3<f32>(0.82, 0.88, 0.98), vec3<f32>(2.2)) * moon_trans;

        let premul_rgb = (disc_lin * disc_mask + halo_lin * halo * (1.0 - disc_mask)) * layer_opacity;
        let alpha = clamp(disc_occlude * layer_opacity, 0.0, 1.0);
        if (alpha <= 1e-4 && max(premul_rgb.r, max(premul_rgb.g, premul_rgb.b)) <= 1e-5) {
            discard;
        }
        return vec4<f32>(premul_rgb, alpha);
    }
}
