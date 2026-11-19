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
    @location(2) cam_z: f32,
}

@vertex
fn vertex(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.uv = input.uv;
    out.alpha = 1.0;

    let v_cam = (uniforms.transform * vec4<f32>(input.position, 0.0)).xyz;
    let v_len = length(v_cam);

    out.clip_position = project_celestial(v_cam, uniforms.projection);
    out.cam_z = select(0.0, v_cam.z / v_len, v_len > 1e-6);
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(tile_texture, tile_sampler, input.uv);
    let min_z = mix(-0.98, 0.02, uniforms.projection.blend_factor);
    if (input.alpha <= 0.0 || input.cam_z < min_z) {
        discard;
    }
    return vec4<f32>(color.rgb, color.a * uniforms.alpha * input.alpha);
}
