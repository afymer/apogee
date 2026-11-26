pub mod center_panel;
pub mod policy_panel;
pub mod queue_panel;
pub mod spreadsheet;
pub mod state;
pub mod timeline;

pub use center_panel::PlanCenterPanel;
pub use policy_panel::PlanPolicyPanel;
pub use queue_panel::PlanQueuePanel;
pub use spreadsheet::SequenceSpreadsheetView;
pub use state::{
    ExploreTargetRequest, GlobalNightPlan, PlanModel, format_duration_compact,
    format_radec_compact, format_unix_hm, is_session_locked, observer_evening_date_and_start,
};
pub use timeline::{NightTimelineView, TimelineWindowMode};

use gpui::{App, AppContext};
use persistence::{DocumentStore, PlannerCatalog};
use workspace::workbench::Workbench;

pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let catalog: PlannerCatalog = store.load_or_default();
    let model = cx.new(|_cx| PlanModel::new(store, catalog));
    cx.set_global(GlobalNightPlan(model));

    cx.observe_new(|workbench: &mut Workbench, window, cx| {
        if let Some(window) = window {
            let queue_panel = PlanQueuePanel::build(window, cx);
            let center_panel = PlanCenterPanel::build(cx);
            let policy_panel = PlanPolicyPanel::build(cx);

            cx.observe(&queue_panel, |_this, _p, cx| cx.notify())
                .detach();
            cx.observe(&center_panel, |_this, _p, cx| cx.notify())
                .detach();
            cx.observe(&policy_panel, |_this, _p, cx| cx.notify())
                .detach();

            workbench.add_panel(queue_panel, window, cx);
            workbench.add_panel(center_panel, window, cx);
            workbench.add_panel(policy_panel, window, cx);
        }
    })
    .detach();
}
