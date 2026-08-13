//! Bounded, authenticated, filesystem-backed audio chunk storage.
//!
//! The vault deliberately exposes audio-specific contracts only. It stores one
//! encrypted file per asset and never accepts a generic file path or media type.
#![forbid(unsafe_code)]

mod contract;
mod crypto;
mod error;
mod format;
mod vault;

pub use contract::{
    AssetId, AudioAssetMetadata, AudioChunk, AudioChunkMetadata, AudioCodec, MasterSecret,
    VaultConfig, VaultLimits,
};
pub use error::VaultError;
pub use vault::{AssetReader, AssetWriter, CleanupReport, Vault, VaultUsage};
