use gpui::Global;

pub(crate) struct AppState {}

impl AppState {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Global for AppState {}
