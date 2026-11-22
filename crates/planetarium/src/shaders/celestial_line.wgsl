#import planetarium::celestial_projection::{CelestialProjectionUniforms, project_celestial}

struct CelestialLineUniforms {
    transform: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    color: vec4<f32>,
    minor_alpha: f32,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialLineUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) line_tier: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) alpha: f32,
    @location(1) tier_alpha: f32,
    @location(2) cam_z: f32,
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let v_cam = (uniforms.transform * vec4<f32>(input.position, 0.0)).xyz;
    let v_len = length(v_cam);

    out.clip_position = project_celestial(v_cam, uniforms.projection);
    out.alpha = 1.0;
    out.tier_alpha = select(
        mix(1.0, uniforms.minor_alpha, input.line_tier),
        clamp(-1.0 - input.line_tier, 0.0, 1.0),
        input.line_tier < -0.5,
    );
    out.cam_z = select(0.0, v_cam.z / v_len, v_len > 1e-6);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let min_z = mix(-0.98, 0.02, uniforms.projection.blend_factor);
    if (input.cam_z < min_z) {
        discard;
    }
    let linear_rgb = pow(max(uniforms.color.rgb, vec3<f32>(0.0)), vec3<f32>(2.2));
    return vec4<f32>(linear_rgb, uniforms.color.a * input.alpha * input.tier_alpha);
}
