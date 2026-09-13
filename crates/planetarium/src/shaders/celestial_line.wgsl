#import planetarium::celestial_projection::{CelestialProjectionUniforms, project_celestial}

struct CelestialLineUniforms {
    transform: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    color: vec4<f32>,
    minor_alpha: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> uniforms: CelestialLineUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
#ifdef VERTEX_LINE_TIER
    @location(1) line_tier: f32,
#endif
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

    // Fade out smoothly if behind camera in perspective mode
    var alpha = 1.0;
    if (uniforms.projection.blend_factor > 0.5 && v_cam.z <= 0.02) {
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.alpha = 0.0;
        out.tier_alpha = 0.0;
        return out;
    }

    out.clip_position = project_celestial(v_cam, uniforms.projection);
    out.alpha = alpha;
#ifdef VERTEX_LINE_TIER
    out.tier_alpha = mix(1.0, uniforms.minor_alpha, input.line_tier);
#else
    out.tier_alpha = 1.0;
#endif
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(uniforms.color.rgb, uniforms.color.a * input.alpha * input.tier_alpha);
}
