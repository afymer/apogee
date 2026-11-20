use thiserror::Error;

/// Errors produced by the persistence layer.
#[derive(Debug, Error)]
pub enum PersistenceError {
    /// Underlying filesystem or IO error.
    #[error("persistence I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization or deserialization error.
    #[error("persistence serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Document key contains unsafe or empty characters.
    #[error("invalid persistence document key: {0:?}")]
    InvalidDocumentKey(String),

    /// On-disk schema version is newer than this binary supports.
    #[error("unsupported schema version {found} (max supported {max_supported})")]
    UnsupportedSchemaVersion { found: u32, max_supported: u32 },

    /// Requested observatory profile ID does not exist.
    #[error("observatory profile {0:?} not found")]
    ProfileNotFound(String),

    /// Cannot delete the sole remaining observatory profile.
    #[error("cannot delete the last remaining observatory profile")]
    CannotDeleteLastProfile,
}
