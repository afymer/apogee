
use gpui::{Anchor, App, IntoElement, div, prelude::*, px, rgb};
use ui::{
    ActiveTheme, ButtonCommon, ButtonLike, ButtonStyle, Color, Icon, IconName, IconSize, Label,
    LabelCommon, LabelSize, PopoverMenu, StyledTypography, h_flex,
};

use crate::icons::{observatory_icon_svg, render_svg_icon};
use crate::popover::ObserverPopover;
use crate::state::GlobalObserver;

/// Expressive title-bar trigger pill and popover menu for the active observatory and time.
pub fn observer_menu(cx: &App) -> impl IntoElement {
    let status = cx.theme().status();

    let (site_name, site_icon, is_live, time_text) =
        if let Some(global) = cx.try_global::<GlobalObserver>() {
            let model = global.0.read(cx);
            let active = model.active_profile();
            let dt = model.effective_local_datetime();
            (
                active.name.clone(),
                active.icon,
                model.clock().is_live(),
                format!("{:02}:{:02}", dt.hour(), dt.minute()),
            )
        } else {
            (
                "Observer".to_string(),
                persistence::ObservatoryIcon::Dome,
                true,
                "--:--".to_string(),
            )
        };

    let trigger = ButtonLike::new("observer_trigger")
        .style(ButtonStyle::Subtle)
        .child(
            h_flex()
                .items_center()
                .gap_1p5()
                .child(render_svg_icon(
                    observatory_icon_svg(site_icon),
                    px(14.0),
                    rgb(0x7be0ec).into(),
                ))
                .child(
                    div()
                        .max_w(px(120.0))
                        .truncate()
                        .child(Label::new(site_name).size(LabelSize::Small)),
                )
                .child(
                    div()
                        .font_buffer(cx)
                        .text_size(px(11.5))
                        .text_color(if is_live {
                            status.success
                        } else {
                            status.warning
                        })
                        .child(time_text),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(IconSize::XSmall)
                        .color(Color::Muted),
                ),
        );

    let menu = PopoverMenu::new("observer_menu")
        .anchor(Anchor::TopRight)
        .attach(Anchor::BottomRight)
        .trigger(trigger)
        .menu(|window, cx| {
            if cx.has_global::<GlobalObserver>() {
                Some(cx.new(|cx| ObserverPopover::new(window, cx)))
            } else {
                None
            }
        });

    // Right-click the trigger to sync back to "now".
    div()
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| {
            cx.stop_propagation();
            if let Some(global) = cx.try_global::<GlobalObserver>().cloned() {
                global.0.update(cx, |model, cx| model.set_live(cx));
            }
        })
        .child(menu)
}
