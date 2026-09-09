// Custom WGSL shader for CR2 RAW display with optional demosaicing

// Static per-image
struct ImageUniforms {
    image_size: vec2<u32>,
    demosaic_enabled: u32,
    _pad0: u32,
    cfa_pattern: vec4<u32>,       // Color channel IDs: (0,0), (0,1), (1,0), (1,1) [0=R, 1=G, 2=B]
    black_levels: vec4<f32>,
    white_levels: vec4<f32>,
    wb_coeffs: vec4<f32>,
};

// Interactive uniforms
// Binding 0: view / display transformation
struct ViewUniforms {
    screen_size: vec2<f32>,
    pan: vec2<f32>,
    zoom: f32,
    _pad0: f32,
    _pad1: vec2<f32>,
};

// Binding 1: tone stretching / display range
struct StretchUniforms {
    range_min_max: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var raw_texture: texture_2d<u32>;
@group(0) @binding(1) var<uniform> img: ImageUniforms;

@group(1) @binding(0) var<uniform> view: ViewUniforms;
@group(1) @binding(1) var<uniform> stretch: StretchUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    // Fullscreen triangle trick: vertices at (-1, -1), (3, -1), (-1, 3)
    let x = f32(i32(in_vertex_index << 1u) & 2) * 2.0 - 1.0;
    let y = f32(i32(in_vertex_index & 2u) * -2) + 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// Fetch raw 16-bit integer sensor value with border clamping
fn get_raw(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(img.image_size.x) - 1);
    let cy = clamp(y, 0, i32(img.image_size.y) - 1);
    return f32(textureLoad(raw_texture, vec2<i32>(cx, cy), 0).r);
}

// Determine CFA color channel for coordinate (x, y): 0=Red, 1=Green, 2=Blue
fn get_cfa_color(x: i32, y: i32) -> u32 {
    let r = u32((y % 2 + 2) % 2);
    let c = u32((x % 2 + 2) % 2);
    let idx = r * 2u + c;
    return img.cfa_pattern[idx];
}

// Subtract black level and scale by white level
fn clean_val(raw_val: f32, color_channel: u32) -> f32 {
    let bl = img.black_levels[color_channel];
    let wl = img.white_levels[color_channel];
    return max((raw_val - bl) / max(wl - bl, 1.0), 0.0);
}

// Interpolate Red or Blue channel using diagonal 4-point, 2-point cross, or direct value
fn interpolate_red_or_blue(x: i32, y: i32, c_center: u32, target_ch: u32, opp_ch: u32) -> f32 {
    if (c_center == target_ch) {
        return clean_val(get_raw(x, y), target_ch);
    } else if (c_center == opp_ch) {
        let ul = clean_val(get_raw(x - 1, y - 1), target_ch);
        let ur = clean_val(get_raw(x + 1, y - 1), target_ch);
        let dl = clean_val(get_raw(x - 1, y + 1), target_ch);
        let dr = clean_val(get_raw(x + 1, y + 1), target_ch);
        return (ul + ur + dl + dr) * 0.25;
    } else {
        // Center is green: check if target color neighbors are horizontal or vertical
        if (get_cfa_color(x - 1, y) == target_ch) {
            let left  = clean_val(get_raw(x - 1, y), target_ch);
            let right = clean_val(get_raw(x + 1, y), target_ch);
            return (left + right) * 0.5;
        } else {
            let up   = clean_val(get_raw(x, y - 1), target_ch);
            let down = clean_val(get_raw(x, y + 1), target_ch);
            return (up + down) * 0.5;
        }
    }
}

// Reconstruct RGB colors using bilinear Bayer demosaicing
fn demosaic_bilinear(x: i32, y: i32, c_center: u32) -> vec3<f32> {
    // 1. Green reconstruction
    var G = 0.0;
    if (c_center == 1u) {
        G = clean_val(get_raw(x, y), 1u);
    } else {
        let g_up    = clean_val(get_raw(x, y - 1), 1u);
        let g_down  = clean_val(get_raw(x, y + 1), 1u);
        let g_left  = clean_val(get_raw(x - 1, y), 1u);
        let g_right = clean_val(get_raw(x + 1, y), 1u);
        G = (g_up + g_down + g_left + g_right) * 0.25;
    }

    // 2. Red & Blue reconstruction
    let R = interpolate_red_or_blue(x, y, c_center, 0u, 2u);
    let B = interpolate_red_or_blue(x, y, c_center, 2u, 0u);

    return vec3<f32>(R, G, B);
}

// Raw Bayer mosaic inspection mode (grayscale sensor values mapped to active channel)
fn get_bayer_mosaic(x: i32, y: i32, c_center: u32) -> vec3<f32> {
    let raw = clean_val(get_raw(x, y), c_center);
    if (c_center == 0u) {
        return vec3<f32>(raw, 0.0, 0.0);
    } else if (c_center == 1u) {
        return vec3<f32>(0.0, raw, 0.0);
    } else {
        return vec3<f32>(0.0, 0.0, raw);
    }
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (img.image_size.x == 0u || img.image_size.y == 0u) {
        return vec4<f32>(0.04, 0.04, 0.05, 1.0);
    }

    // Screen aspect vs Image aspect
    let screen_aspect = view.screen_size.x / max(view.screen_size.y, 1.0);
    let img_aspect = f32(img.image_size.x) / max(f32(img.image_size.y), 1.0);

    var p = in.uv - vec2<f32>(0.5, 0.5);
    if (screen_aspect > img_aspect) {
        p.x *= screen_aspect / img_aspect;
    } else {
        p.y *= img_aspect / screen_aspect;
    }

    let img_uv = (p - view.pan) / max(view.zoom, 1e-4) + vec2<f32>(0.5, 0.5);

    // Outside bounds border check
    if (img_uv.x < 0.0 || img_uv.x >= 1.0 || img_uv.y < 0.0 || img_uv.y >= 1.0) {
        // Dark background for canvas border
        return vec4<f32>(0.04, 0.04, 0.05, 1.0);
    }

    let pixel_coords = img_uv * vec2<f32>(f32(img.image_size.x), f32(img.image_size.y));
    let x = i32(floor(pixel_coords.x));
    let y = i32(floor(pixel_coords.y));

    let c_center = get_cfa_color(x, y);

    var rgb_raw: vec3<f32>;
    if (img.demosaic_enabled == 0u) {
        rgb_raw = get_bayer_mosaic(x, y, c_center);
    } else {
        rgb_raw = demosaic_bilinear(x, y, c_center);
    }

    // Apply camera White Balance multipliers (linear raw display)
    var rgb = rgb_raw * img.wb_coeffs.rgb;

    // Apply min-max range scaling (contrast window)
    let range_min = stretch.range_min_max.x;
    let range_max = max(stretch.range_min_max.y, range_min + 1e-4);
    rgb = clamp((rgb - vec3<f32>(range_min)) / (range_max - range_min), vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(rgb, 1.0);
}
