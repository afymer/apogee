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
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    let v_cam = (uniforms.transform * vec4<f32>(input.position, 0.0)).xyz;

    out.clip_position = project_celestial(v_cam, uniforms.projection);
    out.alpha = 1.0;
    out.tier_alpha = mix(1.0, uniforms.minor_alpha, input.line_tier);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(uniforms.color.rgb, uniforms.color.a * input.alpha * input.tier_alpha);
}
