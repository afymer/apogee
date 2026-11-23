use celestial::HorizonProfile;
use gpui::{App, AppContext, Context, Entity, Global};
use persistence::{
    DEFAULT_HORIZON_SAMPLE_COUNT, DocumentStore, HORIZON_ASSET_NAMESPACE, HorizonBackground,
    HorizonCatalog, PersistenceError,
};

use crate::extraction::PreparedPanorama;

/// Global GPUI entity wrapper for [`HorizonModel`].
#[derive(Clone)]
pub struct GlobalHorizons(pub Entity<HorizonModel>);

impl Global for GlobalHorizons {}

/// Reactive custom horizon & 360° panoramic background state backed by [`DocumentStore`].
pub struct HorizonModel {
    store: DocumentStore,
    catalog: HorizonCatalog,
    live_generation: u64,
}

/// Initializes [`GlobalHorizons`] using the default configuration directory.
pub fn init(cx: &mut App) {
    init_with_store(DocumentStore::open_default(), cx);
}

/// Initializes [`GlobalHorizons`] backed by a specific [`DocumentStore`].
pub fn init_with_store(store: DocumentStore, cx: &mut App) {
    let catalog = store.load_or_default::<HorizonCatalog>();
    let model = cx.new(|_cx| HorizonModel {
        store,
        catalog,
        live_generation: 1,
    });
    cx.set_global(GlobalHorizons(model));
}

impl HorizonModel {
    /// Returns all persisted horizon backgrounds.
    pub fn backgrounds(&self) -> &[HorizonBackground] {
        &self.catalog.backgrounds
    }

    /// Looks up a background by ID.
    pub fn background(&self, id: &str) -> Option<&HorizonBackground> {
        self.catalog.background(id)
    }

    /// Monotonically incremented on every live preview or committed edit.
    pub fn live_generation(&self) -> u64 {
        self.live_generation
    }

    /// Builds a [`HorizonProfile`] for `id` if it exists.
    pub fn profile_for(&self, id: &str) -> Option<HorizonProfile> {
        self.catalog
            .background(id)
            .map(|bg| HorizonProfile::from_samples(bg.samples_deg.clone(), bg.azimuth_offset_deg))
    }

    /// Reads the managed 360° image bytes for `id` from persistence storage.
    pub fn load_image_bytes(&self, id: &str) -> Option<Vec<u8>> {
        let asset = self.catalog.background(id)?.image_asset.as_deref()?;
        match self.store.read_asset(HORIZON_ASSET_NAMESPACE, asset) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!("Failed to read horizon asset {asset:?}: {err}");
                None
            }
        }
    }

    /// Returns the underlying [`DocumentStore`].
    pub fn store(&self) -> &DocumentStore {
        &self.store
    }

    /// Persists a prepared 360° panorama asset, registers the background, and notifies observers.
    pub fn import_prepared(
        &mut self,
        name: &str,
        source_filename: Option<String>,
        prepared: &PreparedPanorama,
        samples_deg: Vec<f32>,
        cx: &mut Context<Self>,
    ) -> Result<String, PersistenceError> {
        let trimmed = name.trim();
        let display_name = if trimmed.is_empty() {
            "New Horizon"
        } else {
            trimmed
        };
        let id = self.catalog.unique_id_for(display_name);
        let asset_name = format!("{id}.{}", prepared.asset_ext);
        self.store.write_asset(
            HORIZON_ASSET_NAMESPACE,
            &asset_name,
            &prepared.managed_bytes,
        )?;

        self.catalog.create_background(
            Some(id.clone()),
            display_name,
            Some(asset_name),
            source_filename,
            Some(prepared.dimensions),
            samples_deg,
            0.0,
        );
        self.persist();
        self.bump_generation();
        cx.notify();
        Ok(id)
    }

    /// Creates a blank flat `0°` horizon entry without a panorama image.
    pub fn create_blank(&mut self, name: &str, cx: &mut Context<Self>) -> String {
        let bg = self.catalog.create_background(
            None,
            name,
            None,
            None,
            None,
            vec![0.0; DEFAULT_HORIZON_SAMPLE_COUNT],
            0.0,
        );
        let id = bg.id.clone();
        self.persist();
        self.bump_generation();
        cx.notify();
        id
    }

    /// Replaces the 360° image asset for an existing background (and optionally updates its curve).
    pub fn replace_prepared_image(
        &mut self,
        id: &str,
        source_filename: Option<String>,
        prepared: &PreparedPanorama,
        extracted_samples: Option<Vec<f32>>,
        cx: &mut Context<Self>,
    ) -> Result<(), PersistenceError> {
        if self.catalog.background(id).is_none() {
            return Err(PersistenceError::HorizonNotFound(id.to_string()));
        }
        let asset_name = format!("{id}.{}", prepared.asset_ext);
        self.store.write_asset(
            HORIZON_ASSET_NAMESPACE,
            &asset_name,
            &prepared.managed_bytes,
        )?;

        let (prev_asset, _) = self.catalog.replace_image(
            id,
            Some(asset_name.clone()),
            source_filename,
            Some(prepared.dimensions),
            extracted_samples,
        )?;
        if let Some(old_asset) = prev_asset
            && old_asset != asset_name
        {
            let _ = self.store.delete_asset(HORIZON_ASSET_NAMESPACE, &old_asset);
        }
        self.persist();
        self.bump_generation();
        cx.notify();
        Ok(())
    }

    /// Renames a background and persists.
    pub fn rename(&mut self, id: &str, name: &str, cx: &mut Context<Self>) -> bool {
        match self.catalog.rename_background(id, name) {
            Ok(_) => {
                self.persist();
                self.bump_generation();
                cx.notify();
                true
            }
            Err(err) => {
                tracing::warn!("Failed to rename horizon {id:?}: {err}");
                false
            }
        }
    }

    /// Updates horizon curve samples and azimuth offset in memory for live previewing.
    pub fn preview_horizon(
        &mut self,
        id: &str,
        samples_deg: Vec<f32>,
        azimuth_offset_deg: f64,
        cx: &mut Context<Self>,
    ) {
        if self
            .catalog
            .set_horizon_in_memory(id, samples_deg, azimuth_offset_deg)
            .is_ok()
        {
            self.bump_generation();
            cx.notify();
        }
    }

    /// Commits horizon curve samples and azimuth offset, bumps revision, and persists to disk.
    pub fn commit_horizon(
        &mut self,
        id: &str,
        samples_deg: Vec<f32>,
        azimuth_offset_deg: f64,
        cx: &mut Context<Self>,
    ) {
        if self
            .catalog
            .commit_horizon(id, samples_deg, azimuth_offset_deg)
            .is_ok()
        {
            self.persist();
            self.bump_generation();
            cx.notify();
        }
    }

    /// Deletes a background and any managed image asset blob.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        match self.catalog.delete_background(id) {
            Ok(asset) => {
                if let Some(asset_name) = asset {
                    let _ = self
                        .store
                        .delete_asset(HORIZON_ASSET_NAMESPACE, &asset_name);
                }
                self.persist();
                self.bump_generation();
                cx.notify();
                true
            }
            Err(err) => {
                tracing::warn!("Failed to delete horizon {id:?}: {err}");
                false
            }
        }
    }

    fn bump_generation(&mut self) {
        self.live_generation = self.live_generation.wrapping_add(1);
    }

    fn persist(&self) {
        if let Err(err) = self.store.save(&self.catalog) {
            tracing::warn!("Failed to persist horizon catalog: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use image::codecs::png::PngEncoder;
    use image::{DynamicImage, RgbaImage};

    use super::*;
    use crate::extraction::decode_and_prepare_panorama;

    fn sample_png_bytes() -> Vec<u8> {
        let img = RgbaImage::from_pixel(64, 32, image::Rgba([100, 150, 220, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_with_encoder(PngEncoder::new(&mut bytes))
            .unwrap();
        bytes
    }

    #[gpui::test]
    fn horizon_model_import_preview_commit_and_delete(cx: &mut TestAppContext) {
        let store = DocumentStore::in_memory();
        let store_for_init = store.clone();

        cx.update(|cx| {
            init_with_store(store_for_init, cx);
        });

        let model = cx.read(|cx| cx.global::<GlobalHorizons>().0.clone());
        let prepared = decode_and_prepare_panorama(&sample_png_bytes()).unwrap();

        let id = model.update(cx, |m, cx| {
            m.import_prepared(
                "Alpine Ridge",
                Some("ridge.jpg".to_string()),
                &prepared,
                vec![8.0; DEFAULT_HORIZON_SAMPLE_COUNT],
                cx,
            )
            .expect("import succeeds")
        });
        assert_eq!(id, "alpine-ridge");

        model.read_with(cx, |m, _| {
            let bg = m.background(&id).unwrap();
            assert_eq!(bg.name, "Alpine Ridge");
            assert_eq!(bg.revision, 1);
            assert_eq!(bg.image_revision, 1);
            assert!(m.load_image_bytes(&id).is_some());
        });

        // Preview updates live_generation and in-memory profile without bumping disk revision.
        let gen_before = model.read_with(cx, |m, _| m.live_generation());
        model.update(cx, |m, cx| {
            m.preview_horizon(&id, vec![14.0; DEFAULT_HORIZON_SAMPLE_COUNT], 25.0, cx);
        });
        model.read_with(cx, |m, _| {
            assert!(m.live_generation() > gen_before);
            let prof = m.profile_for(&id).unwrap();
            assert!((prof.altitude_at_azimuth_deg(25.0) - 14.0).abs() < 1e-4);
            assert_eq!(m.background(&id).unwrap().revision, 1);
        });

        // Commit bumps revision and persists.
        model.update(cx, |m, cx| {
            m.commit_horizon(&id, vec![14.0; DEFAULT_HORIZON_SAMPLE_COUNT], 25.0, cx);
        });
        let persisted: HorizonCatalog = store.load().unwrap();
        assert_eq!(persisted.background(&id).unwrap().revision, 2);
        assert_eq!(persisted.background(&id).unwrap().image_revision, 1);

        // Delete removes catalog entry and image asset.
        let deleted = model.update(cx, |m, cx| m.delete(&id, cx));
        assert!(deleted);
        model.read_with(cx, |m, _| {
            assert_eq!(m.backgrounds(), []);
        });
        assert!(
            store
                .read_asset(HORIZON_ASSET_NAMESPACE, "alpine-ridge.jpg")
                .unwrap()
                .is_none()
        );
    }
}
