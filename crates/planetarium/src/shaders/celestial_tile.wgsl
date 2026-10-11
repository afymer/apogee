#import planetarium::celestial_projection::{CelestialProjectionUniforms, project_celestial}

struct CelestialTileUniforms {
    transform: mat4x4<f32>,
    projection: CelestialProjectionUniforms,
    alpha: f32,
}

@group(0) @binding(0) var<uniform> uniforms: CelestialTileUniforms;
@group(0) @binding(1) var tile_texture: texture_2d<f32>;
@group(0) @binding(2) var tile_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.uv = input.uv;
    out.alpha = 1.0;

    let v_cam = (uniforms.transform * vec4<f32>(input.position, 0.0)).xyz;

    out.clip_position = project_celestial(v_cam, uniforms.projection);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.alpha <= 0.0) {
        discard;
    }
    let color = textureSample(tile_texture, tile_sampler, input.uv);
    return vec4<f32>(color.rgb, color.a * uniforms.alpha * input.alpha);
}
