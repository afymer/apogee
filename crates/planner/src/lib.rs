pub mod center_panel;
pub mod library_panel;
pub mod policy_panel;
pub mod spreadsheet;
pub mod state;
pub mod timeline;

pub use center_panel::PlanCenterPanel;
pub use library_panel::PlanLibraryPanel;
pub use policy_panel::PlanPolicyPanel;
pub use spreadsheet::SequenceSpreadsheetView;
pub use state::{
    ExploreTargetRequest, GlobalNightPlan, PlanModel, format_duration_compact,
    format_radec_compact, format_unix_hm, is_session_locked, observer_evening_date_and_start,
};
pub use timeline::{NightTimelineView, TimelineWindowMode};

use gpui::{AnyElement, App, AppContext, Window, div, prelude::*, px};
use persistence::{DocumentStore, PlannerCatalog};
use ui::{ActiveTheme, Switch, SwitchColor, ToggleState, h_flex};
use workspace::workbench::Workbench;

pub(crate) fn render_switch_row(
    id: &'static str,
    label: &'static str,
    checked: bool,
    disabled: bool,
    on_click: impl Fn(&ToggleState, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .py_0p5()
        .when(disabled, |el| el.opacity(0.55))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(if checked {
                    colors.text
                } else {
                    colors.text_muted
                })
                .child(label),
        )
        .child(
            Switch::new(id, ToggleState::from(checked))
                .color(SwitchColor::Accent)
                .disabled(disabled)
                .on_click(on_click),
        )
        .into_any_element()
}

pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let catalog: PlannerCatalog = store.load_or_default();
    let model = cx.new(|_cx| PlanModel::new(store, catalog));
    cx.set_global(GlobalNightPlan(model));

    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let library_panel = PlanLibraryPanel::build(window, cx);
            let center_panel = PlanCenterPanel::build(cx);
            let policy_panel = PlanPolicyPanel::build(window, cx);

            cx.observe(&library_panel, |_this, _p, cx| cx.notify())
                .detach();
            cx.observe(&center_panel, |_this, _p, cx| cx.notify())
                .detach();
            cx.observe(&policy_panel, |_this, _p, cx| cx.notify())
                .detach();

            workbench.add_panel(library_panel, window, cx);
            workbench.add_panel(center_panel, window, cx);
            workbench.add_panel(policy_panel, window, cx);
        }
    })
    .detach();
}
