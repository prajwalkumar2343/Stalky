use std::path::PathBuf;

use thiserror::Error;

/// Errors returned by configuration, contract validation, encryption, and vault IO.
#[derive(Debug, Error)]
pub enum VaultError {
    #[error("invalid vault configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid asset id")]
    InvalidAssetId,

    #[error("invalid audio metadata: {0}")]
    InvalidAudioMetadata(&'static str),

    #[error("audio chunk is empty")]
    EmptyChunk,

    #[error("audio chunk is {actual} bytes, maximum is {maximum} bytes")]
    ChunkTooLarge { actual: usize, maximum: u32 },

    #[error("asset has reached its maximum chunk count")]
    ChunkCountExceeded,

    #[error("asset size would be {requested} bytes, maximum is {maximum} bytes")]
    AssetTooLarge { requested: u64, maximum: u64 },

    #[error("vault quota would be {requested} bytes, quota is {quota} bytes")]
    QuotaExceeded { requested: u64, quota: u64 },

    #[error("asset not found")]
    NotFound,

    #[error("asset already exists")]
    AlreadyExists,

    #[error("path is outside the canonical audio vault: {path}")]
    UnsafePath { path: PathBuf },

    #[error("asset is corrupt: {0}")]
    Corrupt(&'static str),

    #[error("asset authentication failed")]
    AuthenticationFailed,

    #[error("cryptographic key derivation failed")]
    KeyDerivation,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
