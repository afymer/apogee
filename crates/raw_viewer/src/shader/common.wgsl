// Common WGSL types and sensor calibration functions shared across viewer and compute shaders

struct ImageUniforms {
    image_size: vec2<u32>,
    demosaic_enabled: u32,
    pad0: u32,
    cfa_pattern: vec4<u32>, // Color channel IDs: (0,0), (0,1), (1,0), (1,1) [0=R, 1=G, 2=B]
    black_levels: vec4<f32>,
    white_levels: vec4<f32>,
    wb_coeffs: vec4<f32>,
};

// Determine CFA color channel for coordinate (x, y): 0=Red, 1=Green, 2=Blue
fn get_cfa_color_at(x: i32, y: i32, cfa_pattern: vec4<u32>) -> u32 {
    let r = u32((y % 2 + 2) % 2);
    let c = u32((x % 2 + 2) % 2);
    let idx = r * 2u + c;
    return cfa_pattern[idx];
}

// Subtract sensor black level and scale by white level range
fn calibrate_raw_val(raw_val: f32, color_channel: u32, black_levels: vec4<f32>, white_levels: vec4<f32>) -> f32 {
    let bl = black_levels[color_channel];
    let wl = white_levels[color_channel];
    return max((raw_val - bl) / max(wl - bl, 1.0), 0.0);
}

// Calibrate sensor value and apply camera white balance multiplier
fn calibrate_and_wb_val(raw_val: f32, color_channel: u32, black_levels: vec4<f32>, white_levels: vec4<f32>, wb_coeffs: vec4<f32>) -> f32 {
    let norm = calibrate_raw_val(raw_val, color_channel, black_levels, white_levels);
    return norm * wb_coeffs[color_channel];
}

// Calculate perceptual luminance from linear RGB (BT.709)
fn rgb_to_luminance(r: f32, g: f32, b: f32) -> f32 {
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
