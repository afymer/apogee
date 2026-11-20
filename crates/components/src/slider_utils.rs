use gpui::{Bounds, Hsla, Pixels, Point, hsla};

pub(crate) fn smoothstep_proximity(mouse_pos: Point<Pixels>, bounds: Bounds<Pixels>) -> f32 {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return 0.0;
    }

    let mouse_x = f32::from(mouse_pos.x);
    let mouse_y = f32::from(mouse_pos.y);
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);

    let bar_y = origin_y + height / 2.0;
    let dy = (mouse_y - bar_y).abs();

    let dx = if mouse_x < origin_x {
        origin_x - mouse_x
    } else if mouse_x > origin_x + width {
        mouse_x - (origin_x + width)
    } else {
        0.0
    };

    let dist = (dx * dx + dy * dy).sqrt();

    let d_inner = 3.0;
    let d_outer = 20.0;

    if dist <= d_inner {
        1.0
    } else if dist >= d_outer {
        0.0
    } else {
        let u = (d_outer - dist) / (d_outer - d_inner);
        u * u * (3.0 - 2.0 * u)
    }
}

pub(crate) fn normalized_x(position: Point<Pixels>, bounds: Bounds<Pixels>, knob_radius: f32) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    let usable_width = (width - 2.0 * knob_radius).max(1.0);
    let mouse_x = f32::from(position.x);
    let origin_x = f32::from(bounds.origin.x);
    ((mouse_x - origin_x - knob_radius) / usable_width).clamp(0.0, 1.0)
}

pub(crate) fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    hsla(
        a.h + (b.h - a.h) * t,
        a.s + (b.s - a.s) * t,
        a.l + (b.l - a.l) * t,
        a.a + (b.a - a.a) * t,
    )
}
