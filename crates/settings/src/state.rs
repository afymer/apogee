use gpui::{App, AppContext, Context, Entity, Global};
use persistence::{
    AppSettings, AtmosphereSettings, DEFAULT_ATMOSPHERE_STRENGTH, DEFAULT_DSO_OPACITY,
    DEFAULT_GRID_OPACITY, DEFAULT_GROUND_OPACITY, DEFAULT_HIPS_SURVEY_OPACITY,
    DEFAULT_HORIZON_OPACITY, DEFAULT_SOLAR_SYSTEM_OPACITY, DEFAULT_STARS_OPACITY,
    DEFAULT_TRAJECTORIES_OPACITY, DebugSettings, DocumentStore, DsoSettings, GridLayerSettings,
    GroundSettings, HorizonLayerSettings, KeymapSettings, PlanetariumSettings, SolarSystemSettings,
    StarsSettings, SurveySettings, TrajectoriesSettings,
};

/// Global GPUI entity wrapper for [`SettingsModel`].
#[derive(Clone)]
pub struct GlobalSettings(pub Entity<SettingsModel>);

impl Global for GlobalSettings {}

/// Reactive application settings state backed by [`DocumentStore`].
pub struct SettingsModel {
    store: DocumentStore,
    settings: AppSettings,
    frame_profiler_open: bool,
}

/// Initializes [`GlobalSettings`] using the default configuration directory.
pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

/// Initializes [`GlobalSettings`] backed by a specific [`DocumentStore`].
pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let settings = store.load_or_default::<AppSettings>();
    crate::shortcuts::init(cx);
    crate::shortcuts::sync_keybindings(&settings, cx);
    let model = cx.new(|_cx| SettingsModel {
        store,
        settings,
        frame_profiler_open: false,
    });
    cx.set_global(GlobalSettings(model));
}

impl SettingsModel {
    /// Returns the full settings document.
    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// Returns the planetarium settings section.
    pub fn planetarium(&self) -> PlanetariumSettings {
        self.settings.planetarium
    }

    /// Returns the atmosphere settings.
    pub fn atmosphere(&self) -> AtmosphereSettings {
        self.settings.planetarium.atmosphere
    }

    /// Returns whether the day/twilight atmosphere shader is enabled.
    pub fn atmosphere_enabled(&self) -> bool {
        self.settings.planetarium.atmosphere.enabled
    }

    /// Returns the configured atmosphere strength in `[0.0, 1.0]`.
    pub fn atmosphere_strength(&self) -> f32 {
        self.settings.planetarium.atmosphere.strength
    }

    /// Returns the effective atmosphere strength (`0.0` when disabled).
    pub fn effective_atmosphere_strength(&self) -> f32 {
        self.settings.planetarium.atmosphere.effective_strength()
    }

    /// Sets whether the day/twilight atmosphere shader is enabled and persists the change.
    pub fn set_atmosphere_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.atmosphere.enabled = enabled,
            cx,
        );
    }

    /// Updates atmosphere strength in memory during interactive slider dragging without persisting.
    pub fn preview_atmosphere_strength(&mut self, strength: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(strength, DEFAULT_ATMOSPHERE_STRENGTH);
        if (self.settings.planetarium.atmosphere.strength - clamped).abs() > 1e-6 {
            self.settings.planetarium.atmosphere.strength = clamped;
            cx.notify();
        }
    }

    /// Sets atmosphere strength, persists immediately to disk, and notifies observers.
    pub fn commit_atmosphere_strength(&mut self, strength: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(strength, DEFAULT_ATMOSPHERE_STRENGTH);
        self.settings.planetarium.atmosphere.strength = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the ground mask settings.
    pub fn ground(&self) -> GroundSettings {
        self.settings.planetarium.ground
    }

    /// Returns whether the below-horizon ground mask is enabled.
    pub fn ground_enabled(&self) -> bool {
        self.settings.planetarium.ground.enabled
    }

    /// Returns the configured ground mask opacity in `[0.0, 1.0]`.
    pub fn ground_opacity(&self) -> f32 {
        self.settings.planetarium.ground.opacity
    }

    /// Returns the effective ground mask opacity (`0.0` when disabled).
    pub fn effective_ground_opacity(&self) -> f32 {
        self.settings.planetarium.ground.effective_opacity()
    }

    /// Sets whether the below-horizon ground mask is enabled and persists the change.
    pub fn set_ground_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(|settings| settings.planetarium.ground.enabled = enabled, cx);
    }

    /// Updates ground mask opacity in memory during interactive slider dragging without persisting.
    pub fn preview_ground_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GROUND_OPACITY);
        if (self.settings.planetarium.ground.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.ground.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets ground mask opacity, persists immediately to disk, and notifies observers.
    pub fn commit_ground_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GROUND_OPACITY);
        self.settings.planetarium.ground.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the custom horizon & background panorama settings.
    pub fn horizon(&self) -> HorizonLayerSettings {
        self.settings.planetarium.horizon
    }

    /// Returns whether the custom horizon & background layer is enabled.
    pub fn horizon_enabled(&self) -> bool {
        self.settings.planetarium.horizon.enabled
    }

    /// Returns the configured custom horizon layer opacity in `[0.0, 1.0]`.
    pub fn horizon_opacity(&self) -> f32 {
        self.settings.planetarium.horizon.opacity
    }

    /// Returns the effective custom horizon layer opacity (`0.0` when disabled).
    pub fn effective_horizon_opacity(&self) -> f32 {
        self.settings.planetarium.horizon.effective_opacity()
    }

    /// Sets whether the custom horizon & background layer is enabled and persists the change.
    pub fn set_horizon_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.horizon.enabled = enabled,
            cx,
        );
    }

    /// Returns whether the extracted horizon contour line is drawn over 360° panorama images.
    pub fn horizon_show_line(&self) -> bool {
        self.settings.planetarium.horizon.show_line
    }

    /// Sets whether the horizon contour line is drawn over 360° panorama images and persists.
    pub fn set_horizon_show_line(&mut self, show: bool, cx: &mut Context<Self>) {
        self.update(|settings| settings.planetarium.horizon.show_line = show, cx);
    }

    /// Updates custom horizon opacity in memory during interactive slider dragging without persisting.
    pub fn preview_horizon_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_HORIZON_OPACITY);
        if (self.settings.planetarium.horizon.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.horizon.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets custom horizon opacity, persists immediately to disk, and notifies observers.
    pub fn commit_horizon_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_HORIZON_OPACITY);
        self.settings.planetarium.horizon.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the HiPS background survey settings.
    pub fn hips_survey(&self) -> SurveySettings {
        self.settings.planetarium.hips_survey
    }

    /// Returns whether the HiPS background survey layer is enabled.
    pub fn hips_survey_enabled(&self) -> bool {
        self.settings.planetarium.hips_survey.enabled
    }

    /// Returns the configured HiPS survey opacity in `[0.0, 1.0]`.
    pub fn hips_survey_opacity(&self) -> f32 {
        self.settings.planetarium.hips_survey.opacity
    }

    /// Returns the effective HiPS survey opacity (`0.0` when disabled).
    pub fn effective_hips_survey_opacity(&self) -> f32 {
        self.settings.planetarium.hips_survey.effective_opacity()
    }

    /// Sets whether the HiPS background survey layer is enabled and persists the change.
    pub fn set_hips_survey_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.hips_survey.enabled = enabled,
            cx,
        );
    }

    /// Updates HiPS survey opacity in memory during interactive slider dragging without persisting.
    pub fn preview_hips_survey_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_HIPS_SURVEY_OPACITY);
        if (self.settings.planetarium.hips_survey.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.hips_survey.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets HiPS survey opacity, persists immediately to disk, and notifies observers.
    pub fn commit_hips_survey_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_HIPS_SURVEY_OPACITY);
        self.settings.planetarium.hips_survey.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the catalog stars layer settings.
    pub fn stars(&self) -> StarsSettings {
        self.settings.planetarium.stars
    }

    /// Returns whether catalog star points are enabled.
    pub fn stars_enabled(&self) -> bool {
        self.settings.planetarium.stars.enabled
    }

    /// Returns the configured stars layer opacity in `[0.0, 1.0]`.
    pub fn stars_opacity(&self) -> f32 {
        self.settings.planetarium.stars.opacity
    }

    /// Returns the effective stars layer opacity (`0.0` when disabled).
    pub fn effective_stars_opacity(&self) -> f32 {
        self.settings.planetarium.stars.effective_opacity()
    }

    /// Sets whether catalog star points are enabled and persists the change.
    pub fn set_stars_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(|settings| settings.planetarium.stars.enabled = enabled, cx);
    }

    /// Updates stars layer opacity in memory during interactive slider dragging without persisting.
    pub fn preview_stars_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_STARS_OPACITY);
        if (self.settings.planetarium.stars.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.stars.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets stars layer opacity, persists immediately to disk, and notifies observers.
    pub fn commit_stars_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_STARS_OPACITY);
        self.settings.planetarium.stars.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the deep-sky (Messier / NGC) catalog layer settings.
    pub fn dso(&self) -> DsoSettings {
        self.settings.planetarium.deep_sky
    }

    /// Returns whether deep-sky catalog markers and labels are enabled.
    pub fn dso_enabled(&self) -> bool {
        self.settings.planetarium.deep_sky.enabled
    }

    /// Returns the configured deep-sky layer opacity in `[0.0, 1.0]`.
    pub fn dso_opacity(&self) -> f32 {
        self.settings.planetarium.deep_sky.opacity
    }

    /// Returns the effective deep-sky layer opacity (`0.0` when disabled).
    pub fn effective_dso_opacity(&self) -> f32 {
        self.settings.planetarium.deep_sky.effective_opacity()
    }

    /// Sets whether deep-sky catalog markers are enabled and persists the change.
    pub fn set_dso_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.deep_sky.enabled = enabled,
            cx,
        );
    }

    /// Updates deep-sky layer opacity in memory during interactive slider dragging without persisting.
    pub fn preview_dso_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_DSO_OPACITY);
        if (self.settings.planetarium.deep_sky.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.deep_sky.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets deep-sky layer opacity, persists immediately to disk, and notifies observers.
    pub fn commit_dso_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_DSO_OPACITY);
        self.settings.planetarium.deep_sky.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns whether Messier objects are included in the deep-sky layer.
    pub fn dso_show_messier(&self) -> bool {
        self.settings.planetarium.deep_sky.show_messier
    }

    /// Sets whether Messier objects are shown in the deep-sky layer and persists the change.
    pub fn set_dso_show_messier(&mut self, show: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.deep_sky.show_messier = show,
            cx,
        );
    }

    /// Returns whether NGC objects are included in the deep-sky layer.
    pub fn dso_show_ngc(&self) -> bool {
        self.settings.planetarium.deep_sky.show_ngc
    }

    /// Sets whether NGC objects are shown in the deep-sky layer and persists the change.
    pub fn set_dso_show_ngc(&mut self, show: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.deep_sky.show_ngc = show,
            cx,
        );
    }

    /// Returns the solar-system bodies (Sun and Moon) layer settings.
    pub fn solar_system(&self) -> SolarSystemSettings {
        self.settings.planetarium.solar_system
    }

    /// Returns whether solar-system objects (Sun and Moon) are enabled.
    pub fn solar_system_enabled(&self) -> bool {
        self.settings.planetarium.solar_system.enabled
    }

    /// Returns the configured solar-system layer opacity in `[0.0, 1.0]`.
    pub fn solar_system_opacity(&self) -> f32 {
        self.settings.planetarium.solar_system.opacity
    }

    /// Returns the effective solar-system layer opacity (`0.0` when disabled).
    pub fn effective_solar_system_opacity(&self) -> f32 {
        self.settings.planetarium.solar_system.effective_opacity()
    }

    /// Sets whether solar-system objects (Sun and Moon) are enabled and persists the change.
    pub fn set_solar_system_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.solar_system.enabled = enabled,
            cx,
        );
    }

    /// Updates solar-system layer opacity in memory during interactive slider dragging without persisting.
    pub fn preview_solar_system_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_SOLAR_SYSTEM_OPACITY);
        if (self.settings.planetarium.solar_system.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.solar_system.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets solar-system layer opacity, persists immediately to disk, and notifies observers.
    pub fn commit_solar_system_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_SOLAR_SYSTEM_OPACITY);
        self.settings.planetarium.solar_system.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the horizontal (Alt/Az) coordinate grid settings.
    pub fn altaz_grid(&self) -> GridLayerSettings {
        self.settings.planetarium.altaz_grid
    }

    /// Returns whether the Alt/Az coordinate grid is enabled.
    pub fn altaz_grid_enabled(&self) -> bool {
        self.settings.planetarium.altaz_grid.enabled
    }

    /// Returns the configured Alt/Az grid opacity in `[0.0, 1.0]`.
    pub fn altaz_grid_opacity(&self) -> f32 {
        self.settings.planetarium.altaz_grid.opacity
    }

    /// Returns the effective Alt/Az grid opacity (`0.0` when disabled).
    pub fn effective_altaz_grid_opacity(&self) -> f32 {
        self.settings.planetarium.altaz_grid.effective_opacity()
    }

    /// Sets whether the Alt/Az coordinate grid is enabled and persists the change.
    pub fn set_altaz_grid_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.altaz_grid.enabled = enabled,
            cx,
        );
    }

    /// Updates Alt/Az grid opacity in memory during interactive slider dragging without persisting.
    pub fn preview_altaz_grid_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GRID_OPACITY);
        if (self.settings.planetarium.altaz_grid.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.altaz_grid.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets Alt/Az grid opacity, persists immediately to disk, and notifies observers.
    pub fn commit_altaz_grid_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GRID_OPACITY);
        self.settings.planetarium.altaz_grid.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the equatorial (RA/Dec) coordinate grid settings.
    pub fn equatorial_grid(&self) -> GridLayerSettings {
        self.settings.planetarium.equatorial_grid
    }

    /// Returns whether the equatorial coordinate grid is enabled.
    pub fn equatorial_grid_enabled(&self) -> bool {
        self.settings.planetarium.equatorial_grid.enabled
    }

    /// Returns the configured equatorial grid opacity in `[0.0, 1.0]`.
    pub fn equatorial_grid_opacity(&self) -> f32 {
        self.settings.planetarium.equatorial_grid.opacity
    }

    /// Returns the effective equatorial grid opacity (`0.0` when disabled).
    pub fn effective_equatorial_grid_opacity(&self) -> f32 {
        self.settings
            .planetarium
            .equatorial_grid
            .effective_opacity()
    }

    /// Sets whether the equatorial coordinate grid is enabled and persists the change.
    pub fn set_equatorial_grid_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.equatorial_grid.enabled = enabled,
            cx,
        );
    }

    /// Updates equatorial grid opacity in memory during interactive slider dragging without persisting.
    pub fn preview_equatorial_grid_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GRID_OPACITY);
        if (self.settings.planetarium.equatorial_grid.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.equatorial_grid.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets equatorial grid opacity, persists immediately to disk, and notifies observers.
    pub fn commit_equatorial_grid_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_GRID_OPACITY);
        self.settings.planetarium.equatorial_grid.opacity = clamped;
        self.persist();
        cx.notify();
    }

    /// Returns the selected-object trajectory overlay settings.
    pub fn trajectories(&self) -> TrajectoriesSettings {
        self.settings.planetarium.trajectories
    }

    /// Returns whether selected-object trajectories are enabled.
    pub fn trajectories_enabled(&self) -> bool {
        self.settings.planetarium.trajectories.enabled
    }

    /// Returns the configured trajectories opacity in `[0.0, 1.0]`.
    pub fn trajectories_opacity(&self) -> f32 {
        self.settings.planetarium.trajectories.opacity
    }

    /// Returns the effective trajectories opacity (`0.0` when disabled).
    pub fn effective_trajectories_opacity(&self) -> f32 {
        self.settings.planetarium.trajectories.effective_opacity()
    }

    /// Sets whether selected-object trajectories are enabled and persists the change.
    pub fn set_trajectories_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update(
            |settings| settings.planetarium.trajectories.enabled = enabled,
            cx,
        );
    }

    /// Updates trajectories opacity in memory during interactive slider dragging without persisting.
    pub fn preview_trajectories_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_TRAJECTORIES_OPACITY);
        if (self.settings.planetarium.trajectories.opacity - clamped).abs() > 1e-6 {
            self.settings.planetarium.trajectories.opacity = clamped;
            cx.notify();
        }
    }

    /// Sets trajectories opacity, persists immediately to disk, and notifies observers.
    pub fn commit_trajectories_opacity(&mut self, opacity: f32, cx: &mut Context<Self>) {
        let clamped = sanitize_unit(opacity, DEFAULT_TRAJECTORIES_OPACITY);
        self.settings.planetarium.trajectories.opacity = clamped;
        self.persist();
        cx.notify();
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

    /// Returns whether the frame time debug toggle button is visible in the status bar.
    pub fn show_frame_profiler(&self) -> bool {
        self.settings.debug.show_frame_profiler
    }

    /// Sets whether the frame time debug toggle button is visible in the status bar and persists the change.
    pub fn set_show_frame_profiler(&mut self, show: bool, cx: &mut Context<Self>) {
        let closed_overlay = if !show && self.frame_profiler_open {
            self.frame_profiler_open = false;
            true
        } else {
            false
        };
        let was_persisted = self.settings.debug.show_frame_profiler;
        self.update(|settings| settings.debug.show_frame_profiler = show, cx);
        if closed_overlay && was_persisted == show {
            cx.notify();
        }
    }

    /// Toggles the status bar frame time debug button visibility and persists the change.
    pub fn toggle_show_frame_profiler(&mut self, cx: &mut Context<Self>) {
        let next = !self.settings.debug.show_frame_profiler;
        self.set_show_frame_profiler(next, cx);
    }

    /// Returns whether the runtime frame time diagnostic overlay is currently open.
    pub fn is_frame_profiler_open(&self) -> bool {
        self.frame_profiler_open
    }

    /// Opens or closes the runtime frame time diagnostic overlay without persisting to disk.
    pub fn set_frame_profiler_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.frame_profiler_open != open {
            self.frame_profiler_open = open;
            cx.notify();
        }
    }

    /// Toggles the runtime frame time diagnostic overlay without persisting to disk.
    pub fn toggle_frame_profiler_open(&mut self, cx: &mut Context<Self>) {
        self.set_frame_profiler_open(!self.frame_profiler_open, cx);
    }

    /// Returns the persisted keymap override settings.
    pub fn keymap(&self) -> &KeymapSettings {
        &self.settings.keymap
    }

    /// Returns whether any user keybinding overrides are active.
    pub fn has_keybinding_overrides(&self) -> bool {
        !self.settings.keymap.overrides.is_empty()
    }

    /// Sets or clears (`None` = unbound) a user keybinding override, persists it, and updates GPUI's keymap.
    pub fn set_keybinding_override(
        &mut self,
        action_id: impl Into<String>,
        binding: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let action_id = action_id.into();
        let canonical = binding.and_then(crate::shortcuts::canonicalize_binding);
        let default_canonical = crate::shortcuts::default_binding_for_action(&action_id)
            .and_then(crate::shortcuts::canonicalize_binding);

        self.update(
            |settings| {
                if canonical == default_canonical {
                    settings.keymap.overrides.remove(&action_id);
                } else {
                    settings.keymap.overrides.insert(action_id, canonical);
                }
            },
            cx,
        );
        crate::shortcuts::sync_keybindings(&self.settings, cx);
    }

    /// Resets a single action's keybinding back to its built-in default.
    pub fn reset_keybinding(&mut self, action_id: &str, cx: &mut Context<Self>) {
        self.update(
            |settings| {
                settings.keymap.overrides.remove(action_id);
            },
            cx,
        );
        crate::shortcuts::sync_keybindings(&self.settings, cx);
    }

    /// Resets all user keybinding overrides back to built-in defaults.
    pub fn reset_all_keybindings(&mut self, cx: &mut Context<Self>) {
        self.update(
            |settings| {
                settings.keymap.overrides.clear();
            },
            cx,
        );
        crate::shortcuts::sync_keybindings(&self.settings, cx);
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

fn sanitize_unit(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
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

    #[gpui::test]
    fn settings_model_frame_profiler_persists_and_toggles(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(!m.show_frame_profiler());
            assert!(!m.is_frame_profiler_open());
        });

        // Runtime open/close does not persist to disk.
        model.update(cx, |m, cx| {
            m.toggle_frame_profiler_open(cx);
        });
        model.read_with(cx, |m, _| {
            assert!(m.is_frame_profiler_open());
            assert!(!m.show_frame_profiler());
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.set_show_frame_profiler(true, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(m.show_frame_profiler());
            assert!(m.is_frame_profiler_open());
        });
        let persisted: AppSettings = store.load().expect("persisted profiler settings load");
        assert!(persisted.debug.show_frame_profiler);

        // Disabling the status bar button also closes an open overlay.
        model.update(cx, |m, cx| {
            m.set_show_frame_profiler(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.show_frame_profiler());
            assert!(!m.is_frame_profiler_open());
        });
        let persisted_after_toggle: AppSettings =
            store.load().expect("persisted profiler toggle load");
        assert!(!persisted_after_toggle.debug.show_frame_profiler);
    }

    #[gpui::test]
    fn settings_model_atmosphere_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.atmosphere_enabled());
            assert_eq!(m.atmosphere_strength(), 1.0);
            assert_eq!(m.effective_atmosphere_strength(), 1.0);
        });

        // Preview updates in-memory state without writing to disk.
        model.update(cx, |m, cx| {
            m.preview_atmosphere_strength(0.42, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.atmosphere_strength() - 0.42).abs() < 1e-6);
            assert!((m.effective_atmosphere_strength() - 0.42).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        // Commit with the same previewed value must still persist to disk.
        model.update(cx, |m, cx| {
            m.commit_atmosphere_strength(0.42, cx);
        });
        let persisted: AppSettings = store.load().expect("committed settings load");
        assert!((persisted.planetarium.atmosphere.strength - 0.42).abs() < 1e-6);

        // Disabling atmosphere zeroes effective strength while preserving stored strength.
        model.update(cx, |m, cx| {
            m.set_atmosphere_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.atmosphere_enabled());
            assert!((m.atmosphere_strength() - 0.42).abs() < 1e-6);
            assert_eq!(m.effective_atmosphere_strength(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled settings load");
        assert!(!persisted_disabled.planetarium.atmosphere.enabled);
        assert!((persisted_disabled.planetarium.atmosphere.strength - 0.42).abs() < 1e-6);
    }

    #[gpui::test]
    fn settings_model_ground_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.ground_enabled());
            assert_eq!(m.ground_opacity(), 1.0);
            assert_eq!(m.effective_ground_opacity(), 1.0);
        });

        model.update(cx, |m, cx| {
            m.preview_ground_opacity(0.65, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.ground_opacity() - 0.65).abs() < 1e-6);
            assert!((m.effective_ground_opacity() - 0.65).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_ground_opacity(0.65, cx);
        });
        let persisted: AppSettings = store.load().expect("committed ground load");
        assert!((persisted.planetarium.ground.opacity - 0.65).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_ground_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.ground_enabled());
            assert!((m.ground_opacity() - 0.65).abs() < 1e-6);
            assert_eq!(m.effective_ground_opacity(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled ground load");
        assert!(!persisted_disabled.planetarium.ground.enabled);
        assert!((persisted_disabled.planetarium.ground.opacity - 0.65).abs() < 1e-6);
    }

    #[gpui::test]
    fn settings_model_hips_survey_and_stars_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.hips_survey_enabled());
            assert_eq!(m.hips_survey_opacity(), 1.0);
            assert_eq!(m.effective_hips_survey_opacity(), 1.0);
            assert!(m.stars_enabled());
            assert_eq!(m.stars_opacity(), 1.0);
            assert_eq!(m.effective_stars_opacity(), 1.0);
        });

        model.update(cx, |m, cx| {
            m.preview_hips_survey_opacity(0.35, cx);
            m.preview_stars_opacity(0.55, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.hips_survey_opacity() - 0.35).abs() < 1e-6);
            assert!((m.effective_hips_survey_opacity() - 0.35).abs() < 1e-6);
            assert!((m.stars_opacity() - 0.55).abs() < 1e-6);
            assert!((m.effective_stars_opacity() - 0.55).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_hips_survey_opacity(0.35, cx);
            m.commit_stars_opacity(0.55, cx);
        });
        let persisted: AppSettings = store.load().expect("committed survey & stars load");
        assert!((persisted.planetarium.hips_survey.opacity - 0.35).abs() < 1e-6);
        assert!((persisted.planetarium.stars.opacity - 0.55).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_hips_survey_enabled(false, cx);
            m.set_stars_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.hips_survey_enabled());
            assert!((m.hips_survey_opacity() - 0.35).abs() < 1e-6);
            assert_eq!(m.effective_hips_survey_opacity(), 0.0);
            assert!(!m.stars_enabled());
            assert!((m.stars_opacity() - 0.55).abs() < 1e-6);
            assert_eq!(m.effective_stars_opacity(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled survey & stars load");
        assert!(!persisted_disabled.planetarium.hips_survey.enabled);
        assert!((persisted_disabled.planetarium.hips_survey.opacity - 0.35).abs() < 1e-6);
        assert!(!persisted_disabled.planetarium.stars.enabled);
        assert!((persisted_disabled.planetarium.stars.opacity - 0.55).abs() < 1e-6);
    }

    #[gpui::test]
    fn settings_model_coordinate_grids_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.altaz_grid_enabled());
            assert_eq!(m.altaz_grid_opacity(), 1.0);
            assert_eq!(m.effective_altaz_grid_opacity(), 1.0);
            assert!(m.equatorial_grid_enabled());
            assert_eq!(m.equatorial_grid_opacity(), 1.0);
            assert_eq!(m.effective_equatorial_grid_opacity(), 1.0);
        });

        model.update(cx, |m, cx| {
            m.preview_altaz_grid_opacity(0.4, cx);
            m.preview_equatorial_grid_opacity(0.7, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.altaz_grid_opacity() - 0.4).abs() < 1e-6);
            assert!((m.effective_altaz_grid_opacity() - 0.4).abs() < 1e-6);
            assert!((m.equatorial_grid_opacity() - 0.7).abs() < 1e-6);
            assert!((m.effective_equatorial_grid_opacity() - 0.7).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_altaz_grid_opacity(0.4, cx);
            m.commit_equatorial_grid_opacity(0.7, cx);
        });
        let persisted: AppSettings = store.load().expect("committed grid load");
        assert!((persisted.planetarium.altaz_grid.opacity - 0.4).abs() < 1e-6);
        assert!((persisted.planetarium.equatorial_grid.opacity - 0.7).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_altaz_grid_enabled(false, cx);
            m.set_equatorial_grid_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.altaz_grid_enabled());
            assert!((m.altaz_grid_opacity() - 0.4).abs() < 1e-6);
            assert_eq!(m.effective_altaz_grid_opacity(), 0.0);
            assert!(!m.equatorial_grid_enabled());
            assert!((m.equatorial_grid_opacity() - 0.7).abs() < 1e-6);
            assert_eq!(m.effective_equatorial_grid_opacity(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled grid load");
        assert!(!persisted_disabled.planetarium.altaz_grid.enabled);
        assert!((persisted_disabled.planetarium.altaz_grid.opacity - 0.4).abs() < 1e-6);
        assert!(!persisted_disabled.planetarium.equatorial_grid.enabled);
        assert!((persisted_disabled.planetarium.equatorial_grid.opacity - 0.7).abs() < 1e-6);
    }

    #[gpui::test]
    fn settings_model_horizon_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.horizon_enabled());
            assert_eq!(m.horizon_opacity(), 1.0);
            assert_eq!(m.effective_horizon_opacity(), 1.0);
            assert!(!m.horizon_show_line());
        });

        model.update(cx, |m, cx| {
            m.preview_horizon_opacity(0.55, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.horizon_opacity() - 0.55).abs() < 1e-6);
            assert!((m.effective_horizon_opacity() - 0.55).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_horizon_opacity(0.55, cx);
        });
        let persisted: AppSettings = store.load().expect("committed horizon load");
        assert!((persisted.planetarium.horizon.opacity - 0.55).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_horizon_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.horizon_enabled());
            assert!((m.horizon_opacity() - 0.55).abs() < 1e-6);
            assert_eq!(m.effective_horizon_opacity(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled horizon load");
        assert!(!persisted_disabled.planetarium.horizon.enabled);
        assert!((persisted_disabled.planetarium.horizon.opacity - 0.55).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_horizon_show_line(true, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(m.horizon_show_line());
        });
        let persisted_line: AppSettings = store.load().expect("show_line horizon load");
        assert!(persisted_line.planetarium.horizon.show_line);
    }

    #[gpui::test]
    fn settings_model_dso_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.dso_enabled());
            assert_eq!(m.dso_opacity(), 1.0);
            assert_eq!(m.effective_dso_opacity(), 1.0);
            assert!(m.dso_show_messier());
            assert!(m.dso_show_ngc());
        });

        model.update(cx, |m, cx| {
            m.preview_dso_opacity(0.60, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.dso_opacity() - 0.60).abs() < 1e-6);
            assert!((m.effective_dso_opacity() - 0.60).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_dso_opacity(0.60, cx);
        });
        let persisted: AppSettings = store.load().expect("committed dso load");
        assert!((persisted.planetarium.deep_sky.opacity - 0.60).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_dso_enabled(false, cx);
            m.set_dso_show_ngc(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.dso_enabled());
            assert!((m.dso_opacity() - 0.60).abs() < 1e-6);
            assert_eq!(m.effective_dso_opacity(), 0.0);
            assert!(m.dso_show_messier());
            assert!(!m.dso_show_ngc());
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled dso load");
        assert!(!persisted_disabled.planetarium.deep_sky.enabled);
        assert!(!persisted_disabled.planetarium.deep_sky.show_ngc);
    }

    #[gpui::test]
    fn settings_model_trajectories_preview_commit_and_toggle(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalSettings>().0.clone());
        model.read_with(cx, |m, _| {
            assert!(m.trajectories_enabled());
            assert_eq!(m.trajectories_opacity(), 1.0);
            assert_eq!(m.effective_trajectories_opacity(), 1.0);
        });

        model.update(cx, |m, cx| {
            m.preview_trajectories_opacity(0.45, cx);
        });
        model.read_with(cx, |m, _| {
            assert!((m.trajectories_opacity() - 0.45).abs() < 1e-6);
            assert!((m.effective_trajectories_opacity() - 0.45).abs() < 1e-6);
        });
        assert!(
            store
                .load_optional::<AppSettings>()
                .expect("load_optional succeeds")
                .is_none()
        );

        model.update(cx, |m, cx| {
            m.commit_trajectories_opacity(0.45, cx);
        });
        let persisted: AppSettings = store.load().expect("committed trajectories load");
        assert!((persisted.planetarium.trajectories.opacity - 0.45).abs() < 1e-6);

        model.update(cx, |m, cx| {
            m.set_trajectories_enabled(false, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(!m.trajectories_enabled());
            assert!((m.trajectories_opacity() - 0.45).abs() < 1e-6);
            assert_eq!(m.effective_trajectories_opacity(), 0.0);
        });
        let persisted_disabled: AppSettings = store.load().expect("disabled trajectories load");
        assert!(!persisted_disabled.planetarium.trajectories.enabled);
        assert!((persisted_disabled.planetarium.trajectories.opacity - 0.45).abs() < 1e-6);
    }
}
