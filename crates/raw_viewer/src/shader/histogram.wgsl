// WGSL compute shader for RAW histogram computation, concatenated with common.wgsl

struct HistogramParams {
    x_log: u32,
    y_log: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var raw_texture: texture_2d<u32>;
@group(0) @binding(1) var<uniform> img: ImageUniforms;
@group(0) @binding(2) var<uniform> params: HistogramParams;
@group(0) @binding(3) var<storage, read_write> accum_bins: array<atomic<u32>, 1024>;
@group(0) @binding(4) var<storage, read_write> output_bins: array<f32, 1024>;

var<workgroup> local_bins: array<atomic<u32>, 1024>;

fn map_value_to_bin(val: f32, x_log: u32) -> u32 {
    let t = clamp(val, 0.0, 1.0);
    var norm = t;
    if (x_log != 0u) {
        const C: f32 = 100.0;
        const INV_LOG_C_PLUS_1: f32 = 0.21668499; // 1.0 / log(101.0)
        norm = log(1.0 + C * t) * INV_LOG_C_PLUS_1;
    }
    return clamp(u32(norm * 255.0), 0u, 255u);
}

@compute @workgroup_size(16, 16, 1)
fn cs_main(
    @builtin(local_invocation_index) local_index: u32,
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    // 1. Cooperatively clear local workgroup bins (256 threads * 4 bins = 1024 bins)
    atomicStore(&local_bins[local_index * 4u + 0u], 0u);
    atomicStore(&local_bins[local_index * 4u + 1u], 0u);
    atomicStore(&local_bins[local_index * 4u + 2u], 0u);
    atomicStore(&local_bins[local_index * 4u + 3u], 0u);

    workgroupBarrier();

    // 2. Each thread processes one 2x2 Bayer quad
    let px_x = global_id.x * 2u;
    let px_y = global_id.y * 2u;

    if (px_x + 1u < img.image_size.x && px_y + 1u < img.image_size.y) {
        let p00 = f32(textureLoad(raw_texture, vec2<i32>(i32(px_x), i32(px_y)), 0).r);
        let p10 = f32(textureLoad(raw_texture, vec2<i32>(i32(px_x + 1u), i32(px_y)), 0).r);
        let p01 = f32(textureLoad(raw_texture, vec2<i32>(i32(px_x), i32(px_y + 1u)), 0).r);
        let p11 = f32(textureLoad(raw_texture, vec2<i32>(i32(px_x + 1u), i32(px_y + 1u)), 0).r);

        let c00 = img.cfa_pattern[0];
        let c10 = img.cfa_pattern[1];
        let c01 = img.cfa_pattern[2];
        let c11 = img.cfa_pattern[3];

        let v00 = calibrate_and_wb_val(p00, c00, img.black_levels, img.white_levels, img.wb_coeffs);
        let v10 = calibrate_and_wb_val(p10, c10, img.black_levels, img.white_levels, img.wb_coeffs);
        let v01 = calibrate_and_wb_val(p01, c01, img.black_levels, img.white_levels, img.wb_coeffs);
        let v11 = calibrate_and_wb_val(p11, c11, img.black_levels, img.white_levels, img.wb_coeffs);

        var r = 0.0;
        var g_sum = 0.0;
        var b = 0.0;

        // Accumulate based on CFA color channels
        if (c00 == 0u) { r += v00; } else if (c00 == 1u) { g_sum += v00; } else { b += v00; }
        if (c10 == 0u) { r += v10; } else if (c10 == 1u) { g_sum += v10; } else { b += v10; }
        if (c01 == 0u) { r += v01; } else if (c01 == 1u) { g_sum += v01; } else { b += v01; }
        if (c11 == 0u) { r += v11; } else if (c11 == 1u) { g_sum += v11; } else { b += v11; }

        let g = g_sum * 0.5;
        let luma = rgb_to_luminance(r, g, b);

        let r_bin = map_value_to_bin(r, params.x_log);
        let g_bin = map_value_to_bin(g, params.x_log);
        let b_bin = map_value_to_bin(b, params.x_log);
        let l_bin = map_value_to_bin(luma, params.x_log);

        atomicAdd(&local_bins[0u * 256u + r_bin], 1u);
        atomicAdd(&local_bins[1u * 256u + g_bin], 1u);
        atomicAdd(&local_bins[2u * 256u + b_bin], 1u);
        atomicAdd(&local_bins[3u * 256u + l_bin], 1u);
    }

    workgroupBarrier();

    // 3. Cooperatively write back local bins into global storage buffer
    let idx0 = local_index * 4u + 0u;
    let idx1 = local_index * 4u + 1u;
    let idx2 = local_index * 4u + 2u;
    let idx3 = local_index * 4u + 3u;

    let v0 = atomicLoad(&local_bins[idx0]);
    if (v0 > 0u) { atomicAdd(&accum_bins[idx0], v0); }
    let v1 = atomicLoad(&local_bins[idx1]);
    if (v1 > 0u) { atomicAdd(&accum_bins[idx1], v1); }
    let v2 = atomicLoad(&local_bins[idx2]);
    if (v2 > 0u) { atomicAdd(&accum_bins[idx2], v2); }
    let v3 = atomicLoad(&local_bins[idx3]);
    if (v3 > 0u) { atomicAdd(&accum_bins[idx3], v3); }
}

@compute @workgroup_size(256, 1, 1)
fn cs_postprocess(
    @builtin(global_invocation_id) global_id: vec3<u32>,
) {
    let idx = global_id.x;
    if (idx >= 1024u) {
        return;
    }
    let count = f32(atomicLoad(&accum_bins[idx]));
    if (params.y_log != 0u) {
        output_bins[idx] = log(1.0 + count);
    } else {
        output_bins[idx] = count;
    }
}
