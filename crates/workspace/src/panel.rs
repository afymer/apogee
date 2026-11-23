use gpui::{
    AnyView, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, px,
};

use crate::{WorkspaceKind, dock::DockPosition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    Activate,
    Close,
    Toggle,
}

pub trait Panel: Focusable + EventEmitter<PanelEvent> + Render + Sized + 'static {
    fn persistent_name() -> &'static str;
    fn title(&self, cx: &App) -> SharedString;
    fn position(&self, _cx: &App) -> DockPosition;
    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, _cx: &App) -> bool;

    fn starts_open(&self, _cx: &App) -> bool {
        true
    }

    fn default_size(&self, _cx: &App) -> Pixels {
        px(240.)
    }

    fn min_size(&self, _cx: &App) -> Pixels {
        px(200.)
    }

    fn set_active(&mut self, _active: bool, _cx: &mut Context<Self>) {}
}

pub trait PanelHandle: Send + Sync {
    fn panel_id(&self) -> EntityId;
    fn persistent_name(&self) -> &'static str;
    fn title(&self, cx: &App) -> SharedString;
    fn position(&self, cx: &App) -> DockPosition;
    fn is_enabled_in_workspace(&self, kind: WorkspaceKind, cx: &App) -> bool;
    fn starts_open(&self, cx: &App) -> bool;
    fn default_size(&self, cx: &App) -> Pixels;
    fn min_size(&self, cx: &App) -> Pixels;
    fn set_active(&self, active: bool, cx: &mut App);
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

    fn starts_open(&self, cx: &App) -> bool {
        self.read(cx).starts_open(cx)
    }

    fn default_size(&self, cx: &App) -> Pixels {
        self.read(cx).default_size(cx)
    }

    fn min_size(&self, cx: &App) -> Pixels {
        self.read(cx).min_size(cx)
    }

    fn set_active(&self, active: bool, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, cx));
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }
}
