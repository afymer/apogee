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
    let stereo_factor = 2.0 / (1.0 + nz);

    // Perspective: r_persp * dir = norm.xy / nz
    let persp_factor = 1.0 / max(nz, 0.001);

    let factor = mix(stereo_factor, persp_factor, proj.blend_factor);
    let scale = max(proj.scale_factor, 1e-9);

    let x_ndc = (norm.x * factor) / (proj.aspect_ratio * scale);
    let y_ndc = (norm.y * factor) / scale;

    let theta = acos(nz);
    // Bevy's Reverse-Z depth buffer (near = 1.0, far = 0.0, GreaterEqual compare)
    let z_ndc = 0.95 - 0.45 * clamp(theta / 3.14159265, 0.0, 1.0);

    return vec4<f32>(x_ndc, y_ndc, z_ndc, 1.0);
}
