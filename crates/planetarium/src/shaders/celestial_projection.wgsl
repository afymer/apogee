#define_import_path planetarium::celestial_projection

struct CelestialProjectionUniforms {
    fov_y: f32,
    aspect_ratio: f32,
    blend_factor: f32,
    scale_factor: f32,
}

// Projects a camera-space 3D direction vector (+Z = optical axis / forward, +X = right, +Y = up)
// into screen space NDC (x ∈ [-1, 1], y ∈ [-1, 1], z ∈ [0.5, 0.95] Reverse-Z).
fn project_celestial(v_cam: vec3<f32>, proj: CelestialProjectionUniforms) -> vec4<f32> {
    let norm = normalize(v_cam);
    let nz = clamp(norm.z, -1.0, 1.0);

    // Stereographic: r_stereo * dir = 2.0 * norm.xy / (1.0 + nz)
    let stereo_factor = 2.0 / max(1.0 + nz, 0.002);

    // Perspective: r_persp * dir = norm.xy / nz
    let persp_factor = 1.0 / max(nz, 0.005);

    let factor = mix(stereo_factor, persp_factor, proj.blend_factor);
    let scale = max(proj.scale_factor, 1e-9);

    let x_ndc = (norm.x * factor) / (proj.aspect_ratio * scale);
    let y_ndc = (norm.y * factor) / scale;

    let theta = acos(nz);
    // Bevy's Reverse-Z depth buffer (near = 1.0, far = 0.0, GreaterEqual compare)
    let z_ndc = 0.95 - 0.45 * clamp(theta / 3.14159265, 0.0, 1.0);

    return vec4<f32>(x_ndc, y_ndc, z_ndc, 1.0);
}

// Reconstructs a unit camera-space 3D direction vector from screen space NDC ([-1, 1]^2).
// Exact inverse of `project_celestial` across stereographic, blended, and perspective projections.
fn unproject_celestial(ndc: vec2<f32>, proj: CelestialProjectionUniforms) -> vec3<f32> {
    let scale = max(proj.scale_factor, 1e-9);
    let p = vec2<f32>(ndc.x * proj.aspect_ratio * scale, ndc.y * scale);
    let r2 = dot(p, p);
    if (r2 <= 1e-12) {
        return vec3<f32>(0.0, 0.0, 1.0);
    }

    let b = clamp(proj.blend_factor, 0.0, 1.0);
    if (b <= 1e-5) {
        let t2 = 0.25 * r2;
        let inv = 1.0 / (1.0 + t2);
        return vec3<f32>(p * inv, (1.0 - t2) * inv);
    }
    if (b >= 1.0 - 1e-5) {
        return normalize(vec3<f32>(p.x, p.y, 1.0));
    }

    let r = sqrt(r2);
    var t = mix(min(0.5 * r, 0.999), r / (1.0 + sqrt(1.0 + r2)), b);
    for (var i = 0; i < 3; i = i + 1) {
        let t2 = t * t;
        let denom = max(1.0 - t2, 1e-4);
        let f = 2.0 * (1.0 - b) * t + (2.0 * b * t) / denom - r;
        let df = 2.0 * (1.0 - b) + (2.0 * b * (1.0 + t2)) / (denom * denom);
        t = clamp(t - f / max(df, 1e-6), 0.0, 0.9999);
    }

    let t2 = t * t;
    let inv = 1.0 / (1.0 + t2);
    return vec3<f32>((p / r) * (2.0 * t * inv), (1.0 - t2) * inv);
}
