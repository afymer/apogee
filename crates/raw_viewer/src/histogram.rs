use std::sync::Arc;

use gpui::{
    App, Context, Entity, IntoElement, MouseButton, PathBuilder, Pixels, Point, Render, Window,
    div, point, prelude::*, px, rgb, rgba,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, prelude::*};

use crate::bridge::RawViewerBridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMode {
    Log,
    Linear,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistogramData {
    pub r: [f32; 256],
    pub g: [f32; 256],
    pub b: [f32; 256],
    pub l: [f32; 256],
    pub max_r: f32,
    pub max_g: f32,
    pub max_b: f32,
    pub max_l: f32,
}

impl HistogramData {
    pub fn from_raw_bins(bins: &[f32; 1024]) -> Self {
        let mut r = [0.0f32; 256];
        let mut g = [0.0f32; 256];
        let mut b = [0.0f32; 256];
        let mut l = [0.0f32; 256];

        r.copy_from_slice(&bins[0..256]);
        g.copy_from_slice(&bins[256..512]);
        b.copy_from_slice(&bins[512..768]);
        l.copy_from_slice(&bins[768..1024]);

        let max_r = r.iter().copied().fold(0.0f32, f32::max);
        let max_g = g.iter().copied().fold(0.0f32, f32::max);
        let max_b = b.iter().copied().fold(0.0f32, f32::max);
        let max_l = l.iter().copied().fold(0.0f32, f32::max);

        Self {
            r,
            g,
            b,
            l,
            max_r,
            max_g,
            max_b,
            max_l,
        }
    }

    #[must_use]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn max_visible(&self, show_r: bool, show_g: bool, show_b: bool, show_l: bool) -> f32 {
        let mut max_val = 0.0f32;
        if show_r {
            max_val = max_val.max(self.max_r);
        }
        if show_g {
            max_val = max_val.max(self.max_g);
        }
        if show_b {
            max_val = max_val.max(self.max_b);
        }
        if show_l {
            max_val = max_val.max(self.max_l);
        }
        max_val.max(1e-6)
    }
}

pub struct HistogramWidget {
    bridge: RawViewerBridge,
    x_scale_mode: ScaleMode,
    y_scale_mode: ScaleMode,
    show_r: bool,
    show_g: bool,
    show_b: bool,
    show_l: bool,
}

impl HistogramWidget {
    pub fn build(bridge: RawViewerBridge, cx: &mut App) -> Entity<Self> {
        let (x_scale_mode, y_scale_mode) = bridge.histogram_scale_modes();
        cx.new(|_cx| Self {
            bridge,
            x_scale_mode,
            y_scale_mode,
            show_r: true,
            show_g: true,
            show_b: true,
            show_l: true,
        })
    }

    pub fn x_scale_mode(&self) -> ScaleMode {
        self.x_scale_mode
    }

    pub fn toggle_x_scale_mode(&mut self, cx: &mut Context<Self>) {
        self.x_scale_mode = match self.x_scale_mode {
            ScaleMode::Log => ScaleMode::Linear,
            ScaleMode::Linear => ScaleMode::Log,
        };
        self.bridge
            .set_histogram_scale_modes(self.x_scale_mode, self.y_scale_mode);
        cx.notify();
    }

    pub fn y_scale_mode(&self) -> ScaleMode {
        self.y_scale_mode
    }

    pub fn toggle_y_scale_mode(&mut self, cx: &mut Context<Self>) {
        self.y_scale_mode = match self.y_scale_mode {
            ScaleMode::Log => ScaleMode::Linear,
            ScaleMode::Linear => ScaleMode::Log,
        };
        self.bridge
            .set_histogram_scale_modes(self.x_scale_mode, self.y_scale_mode);
        cx.notify();
    }

    pub fn scale_mode(&self) -> ScaleMode {
        self.y_scale_mode
    }

    pub fn toggle_scale_mode(&mut self, cx: &mut Context<Self>) {
        self.toggle_y_scale_mode(cx);
    }

    pub fn toggle_channel(&mut self, channel: u8, cx: &mut Context<Self>) {
        match channel {
            0 => self.show_r = !self.show_r,
            1 => self.show_g = !self.show_g,
            2 => self.show_b = !self.show_b,
            3 => self.show_l = !self.show_l,
            _ => {}
        }
        cx.notify();
    }
}

impl Render for HistogramWidget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let histogram_data: Option<Arc<HistogramData>> = self.bridge.histogram();
        let x_scale_mode = self.x_scale_mode;
        let y_scale_mode = self.y_scale_mode;
        let show_r = self.show_r;
        let show_g = self.show_g;
        let show_b = self.show_b;
        let show_l = self.show_l;

        let mut root = div()
            .id("histogram_widget_root")
            .w(px(290.0))
            .p_2()
            .gap_1p5()
            .rounded_md()
            .bg(cx.theme().colors().elevated_surface_background)
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        let x_scale_label = if x_scale_mode == ScaleMode::Log {
            "X: Log"
        } else {
            "X: Lin"
        };
        let y_scale_label = if y_scale_mode == ScaleMode::Log {
            "Y: Log"
        } else {
            "Y: Lin"
        };

        let header = h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Label::new("Hist")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new("hist_x_scale_btn", x_scale_label)
                            .style(if x_scale_mode == ScaleMode::Log {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_x_scale_mode(cx);
                            })),
                    )
                    .child(
                        Button::new("hist_y_scale_btn", y_scale_label)
                            .style(if y_scale_mode == ScaleMode::Log {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_y_scale_mode(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("hist_toggle_r", "R")
                            .style(if show_r {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_channel(0, cx);
                            })),
                    )
                    .child(
                        Button::new("hist_toggle_g", "G")
                            .style(if show_g {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_channel(1, cx);
                            })),
                    )
                    .child(
                        Button::new("hist_toggle_b", "B")
                            .style(if show_b {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_channel(2, cx);
                            })),
                    )
                    .child(
                        Button::new("hist_toggle_l", "L")
                            .style(if show_l {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Subtle
                            })
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.toggle_channel(3, cx);
                            })),
                    ),
            );

        root = root.child(header);

        let hist_clone = histogram_data.clone();
        let chart_canvas = gpui::canvas(
            move |_bounds, _window, _cx| (),
            move |bounds, (), window, _cx| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w <= 0.0 || h <= 0.0 {
                    return;
                }

                let grid_color = rgba(0xffffff0d);
                for factor in [0.25f32, 0.5, 0.75] {
                    let y = bounds.origin.y + px(h * factor);
                    window.paint_quad(gpui::fill(
                        gpui::Bounds {
                            origin: Point::new(bounds.origin.x, y),
                            size: gpui::Size {
                                width: px(w),
                                height: px(1.0),
                            },
                        },
                        grid_color,
                    ));
                }

                let vert_factors: &[f32] = match x_scale_mode {
                    ScaleMode::Linear => &[0.25, 0.5, 0.75],
                    ScaleMode::Log => &[0.01, 0.05, 0.20, 0.50],
                };
                for &t in vert_factors {
                    let norm_x = match x_scale_mode {
                        ScaleMode::Linear => t,
                        ScaleMode::Log => {
                            const C: f32 = 100.0;
                            (1.0 + C * t).ln() / (1.0 + C).ln()
                        }
                    };
                    let x = bounds.origin.x + px(norm_x.clamp(0.0, 1.0) * w);
                    window.paint_quad(gpui::fill(
                        gpui::Bounds {
                            origin: Point::new(x, bounds.origin.y),
                            size: gpui::Size {
                                width: px(1.0),
                                height: px(h),
                            },
                        },
                        grid_color,
                    ));
                }

                let Some(data) = hist_clone.as_ref() else {
                    return;
                };

                let max_count = data.max_visible(show_r, show_g, show_b, show_l);

                let calc_y = |val: f32| -> Pixels {
                    let norm = if max_count > 0.0 {
                        (val / max_count).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    bounds.origin.y + px(h - (norm * (h - 2.0)))
                };

                let calc_x = |i: usize| -> Pixels {
                    let t = i as f32 / 255.0;
                    bounds.origin.x + px(t * w)
                };

                let draw_channel_fill =
                    |bins: &[f32; 256], fill_color: gpui::Rgba, window: &mut Window| {
                        let mut builder = PathBuilder::fill();
                        builder.move_to(point(calc_x(0), bounds.origin.y + px(h)));
                        for i in 0..256 {
                            builder.line_to(point(calc_x(i), calc_y(bins[i])));
                        }
                        builder.line_to(point(calc_x(255), bounds.origin.y + px(h)));
                        builder.line_to(point(calc_x(0), bounds.origin.y + px(h)));
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, fill_color);
                        }
                    };

                let draw_channel_stroke =
                    |bins: &[f32; 256],
                     stroke_color: gpui::Rgba,
                     line_w: f32,
                     window: &mut Window| {
                        let mut builder = PathBuilder::stroke(px(line_w));
                        builder.move_to(point(calc_x(0), calc_y(bins[0])));
                        for i in 1..256 {
                            builder.line_to(point(calc_x(i), calc_y(bins[i])));
                        }
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, stroke_color);
                        }
                    };

                if show_r {
                    draw_channel_fill(&data.r, rgba(0xef444433), window);
                    draw_channel_stroke(&data.r, rgba(0xef444488), 1.0, window);
                }

                if show_g {
                    draw_channel_fill(&data.g, rgba(0x22c55e33), window);
                    draw_channel_stroke(&data.g, rgba(0x22c55e88), 1.0, window);
                }

                if show_b {
                    draw_channel_fill(&data.b, rgba(0x3b82f633), window);
                    draw_channel_stroke(&data.b, rgba(0x3b82f688), 1.0, window);
                }

                if show_l {
                    draw_channel_stroke(&data.l, rgba(0xffffffdd), 1.5, window);
                }
            },
        )
        .size_full();

        let canvas_container = div()
            .w_full()
            .h(px(100.0))
            .rounded_sm()
            .bg(rgb(0x0e0e12))
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .overflow_hidden()
            .child(chart_canvas);

        root = root.child(canvas_container);

        root
    }
}
