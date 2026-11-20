use gpui::{App, AppContext, Context, Entity, Global};
use persistence::{AppSettings, DebugSettings, DocumentStore};

/// Global GPUI entity wrapper for [`SettingsModel`].
#[derive(Clone)]
pub struct GlobalSettings(pub Entity<SettingsModel>);

impl Global for GlobalSettings {}

/// Reactive application settings state backed by [`DocumentStore`].
pub struct SettingsModel {
    store: DocumentStore,
    settings: AppSettings,
}

/// Initializes [`GlobalSettings`] using the default configuration directory.
pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

/// Initializes [`GlobalSettings`] backed by a specific [`DocumentStore`].
pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let settings = store.load_or_default::<AppSettings>();
    let model = cx.new(|_cx| SettingsModel { store, settings });
    cx.set_global(GlobalSettings(model));
}

impl SettingsModel {
    /// Returns the full settings document.
    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// Returns the debug settings section.
    pub fn debug(&self) -> DebugSettings {
        self.settings.debug
    }

    /// Returns whether the green HiPS grid debug lines should be displayed.
    pub fn show_hips_grid(&self) -> bool {
        self.settings.debug.show_hips_grid
    }

    /// Sets whether the green HiPS grid debug lines are displayed and persists the change.
    pub fn set_show_hips_grid(&mut self, show: bool, cx: &mut Context<Self>) {
        self.update(|settings| settings.debug.show_hips_grid = show, cx);
    }

    /// Toggles the green HiPS grid debug lines visibility and persists the change.
    pub fn toggle_show_hips_grid(&mut self, cx: &mut Context<Self>) {
        let next = !self.settings.debug.show_hips_grid;
        self.set_show_hips_grid(next, cx);
    }

    /// Mutates settings, persists atomically to disk if changed, and notifies observers.
    pub fn update(&mut self, mutate: impl FnOnce(&mut AppSettings), cx: &mut Context<Self>) {
        let mut next = self.settings.clone();
        mutate(&mut next);
        if next != self.settings {
            self.settings = next;
            self.persist();
            cx.notify();
        }
    }

    fn persist(&self) {
        if let Err(err) = self.store.save(&self.settings) {
            tracing::warn!("Failed to persist application settings: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn settings_model_mutates_and_persists(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(!m.show_hips_grid());
        });

        model.update(cx, |m, cx| {
            m.set_show_hips_grid(true, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(m.show_hips_grid());
        });

        let persisted: AppSettings = store.load().expect("persisted settings load");
        assert!(persisted.debug.show_hips_grid);

        model.update(cx, |m, cx| {
            m.toggle_show_hips_grid(cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.show_hips_grid());
        });

        let persisted_after_toggle: AppSettings = store.load().expect("persisted settings load");
        assert!(!persisted_after_toggle.debug.show_hips_grid);
    }
}
