use gpui::{
    AnyView, App, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Render, SharedString,
};

use crate::{WorkspaceKind, dock::DockPosition};

pub enum PanelEvent {}

pub trait Panel: Focusable + EventEmitter<PanelEvent> + Render + Sized + 'static {
    fn persistent_name() -> &'static str;
    fn title(&self, cx: &App) -> SharedString;
    fn position(&self, _cx: &App) -> DockPosition;
    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool;
}

pub trait PanelHandle: Send + Sync {
    fn panel_id(&self) -> EntityId;
    fn persistent_name(&self) -> &'static str;
    fn title(&self, cx: &App) -> SharedString;
    fn position(&self, cx: &App) -> DockPosition;
    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, cx: &App) -> bool;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn to_any(&self) -> AnyView;
}

impl<T: Panel> PanelHandle for Entity<T> {
    fn panel_id(&self) -> EntityId {
        self.entity_id()
    }

    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).title(cx)
    }

    fn position(&self, cx: &App) -> DockPosition {
        self.read(cx).position(cx)
    }

    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, cx: &App) -> bool {
        self.read(cx).is_enabled_in_workspace(kind, cx)
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }
}
