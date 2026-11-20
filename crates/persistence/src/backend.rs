use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::error::PersistenceError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Abstract raw byte storage keyed by logical document identifier.
///
/// Implementations may write to local JSON files, memory (for tests),
/// or future synchronized remote storage adapters.
pub trait DocumentBackend: Send + Sync {
    /// Reads raw bytes for `key`, returning `Ok(None)` if no document exists yet.
    fn read_raw(&self, key: &str) -> Result<Option<Vec<u8>>, PersistenceError>;

    /// Atomically persists raw bytes for `key`.
    fn write_raw(&self, key: &str, bytes: &[u8]) -> Result<(), PersistenceError>;
}

/// Validates that `key` is a safe single-segment identifier without path separators.
pub fn validate_key(key: &str) -> Result<(), PersistenceError> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(PersistenceError::InvalidDocumentKey(key.to_string()));
    }
    Ok(())
}

/// Resolves the default directory for persisted Apogee configuration documents.
///
/// Honors `APOGEE_CONFIG_DIR` when non-empty, otherwise uses the platform
/// config directory (`~/.config/apogee` on Linux), falling back to `.apogee`.
pub fn default_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("APOGEE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .map(|base| base.join("apogee"))
        .unwrap_or_else(|| PathBuf::from(".apogee"))
}

/// Local filesystem backend storing each document as `<root>/<key>.json` using
/// atomic temp-file writes and `sync_all` before rename.
#[derive(Debug, Clone)]
pub struct JsonFileBackend {
    root: PathBuf,
}

impl JsonFileBackend {
    /// Creates a backend rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Creates a backend at [`default_config_dir`].
    pub fn default_location() -> Self {
        Self::new(default_config_dir())
    }

    /// Returns the root directory path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves the target JSON file path for `key`.
    pub fn path_for(&self, key: &str) -> Result<PathBuf, PersistenceError> {
        validate_key(key)?;
        Ok(self.root.join(format!("{key}.json")))
    }
}

impl DocumentBackend for JsonFileBackend {
    fn read_raw(&self, key: &str) -> Result<Option<Vec<u8>>, PersistenceError> {
        let path = self.path_for(key)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(PersistenceError::Io(err)),
        }
    }

    fn write_raw(&self, key: &str, bytes: &[u8]) -> Result<(), PersistenceError> {
        let target = self.path_for(key)?;
        std::fs::create_dir_all(&self.root)?;

        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_path = self.root.join(format!(".{key}.json.tmp.{pid}.{seq}"));

        let write_result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp_path, &target)?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PersistenceError::Io(err));
        }

        Ok(())
    }
}

/// Thread-safe in-memory backend useful for headless tests and ephemeral sessions.
#[derive(Debug, Default)]
pub struct MemoryBackend {
    docs: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBackend {
    /// Creates an empty in-memory backend.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DocumentBackend for MemoryBackend {
    fn read_raw(&self, key: &str) -> Result<Option<Vec<u8>>, PersistenceError> {
        validate_key(key)?;
        let guard = self.docs.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(guard.get(key).cloned())
    }

    fn write_raw(&self, key: &str, bytes: &[u8]) -> Result<(), PersistenceError> {
        validate_key(key)?;
        let mut guard = self.docs.lock().unwrap_or_else(PoisonError::into_inner);
        guard.insert(key.to_string(), bytes.to_vec());
        Ok(())
    }
}
