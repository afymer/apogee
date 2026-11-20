use std::path::PathBuf;
use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};

use crate::backend::{DocumentBackend, JsonFileBackend, MemoryBackend};
use crate::error::PersistenceError;

/// Trait implemented by versioned configuration/state documents managed by [`DocumentStore`].
pub trait Persistable: Serialize + DeserializeOwned + Default {
    /// Unique document key used to locate the stored artifact (e.g. `"observer"`).
    const DOCUMENT_KEY: &'static str;

    /// Maximum schema version understood by this type.
    const SCHEMA_VERSION: u32;

    /// Returns the schema version recorded in the deserialized document.
    fn schema_version(&self) -> u32 {
        Self::SCHEMA_VERSION
    }

    /// Normalizes or repairs invariants immediately after deserialization.
    fn normalize_after_load(&mut self) {}
}

/// High-level typed persistence facade over a [`DocumentBackend`].
#[derive(Clone)]
pub struct DocumentStore {
    backend: Arc<dyn DocumentBackend>,
}

impl DocumentStore {
    /// Opens a store backed by JSON files in [`crate::backend::default_config_dir`].
    pub fn open_default() -> Self {
        Self::with_backend(Arc::new(JsonFileBackend::default_location()))
    }

    /// Opens a store backed by JSON files in `dir`.
    pub fn open_dir(dir: PathBuf) -> Self {
        Self::with_backend(Arc::new(JsonFileBackend::new(dir)))
    }

    /// Creates an isolated in-memory store (no disk I/O).
    pub fn in_memory() -> Self {
        Self::with_backend(Arc::new(MemoryBackend::new()))
    }

    /// Creates a store around an arbitrary [`DocumentBackend`].
    pub fn with_backend(backend: Arc<dyn DocumentBackend>) -> Self {
        Self { backend }
    }

    /// Loads document `T`, returning `Ok(None)` if it has not been saved yet.
    pub fn load_optional<T: Persistable>(&self) -> Result<Option<T>, PersistenceError> {
        let Some(bytes) = self.backend.read_raw(T::DOCUMENT_KEY)? else {
            return Ok(None);
        };

        let mut doc: T = serde_json::from_slice(&bytes)?;
        let found = doc.schema_version();
        if found > T::SCHEMA_VERSION {
            return Err(PersistenceError::UnsupportedSchemaVersion {
                found,
                max_supported: T::SCHEMA_VERSION,
            });
        }
        doc.normalize_after_load();
        Ok(Some(doc))
    }

    /// Loads document `T`, returning `T::default()` if it does not exist yet.
    pub fn load<T: Persistable>(&self) -> Result<T, PersistenceError> {
        Ok(self.load_optional()?.unwrap_or_default())
    }

    /// Loads document `T`, falling back to `fallback()` when missing or on failure.
    pub fn load_or_else<T: Persistable>(&self, fallback: impl FnOnce() -> T) -> T {
        match self.load_optional::<T>() {
            Ok(Some(doc)) => doc,
            Ok(None) => fallback(),
            Err(err) => {
                tracing::warn!(
                    document = T::DOCUMENT_KEY,
                    "Failed to load persisted document, falling back to default: {err}"
                );
                fallback()
            }
        }
    }

    /// Loads document `T`, logging a warning and returning `T::default()` on failure.
    pub fn load_or_default<T: Persistable>(&self) -> T {
        self.load_or_else(T::default)
    }

    /// Serializes `doc` as pretty JSON and persists it atomically.
    pub fn save<T: Persistable>(&self, doc: &T) -> Result<(), PersistenceError> {
        let bytes = serde_json::to_vec_pretty(doc)?;
        self.backend.write_raw(T::DOCUMENT_KEY, &bytes)
    }
}
