use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::VaultError;

pub(crate) const MAX_CHUNK_BYTES: u32 = 16 * 1024 * 1024;
pub(crate) const MAX_ASSET_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_VAULT_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
pub(crate) const MAX_CHUNKS_PER_ASSET: u32 = 1_000_000;
pub(crate) const MAX_CHUNK_DURATION_MICROS: u32 = 60_000_000;
pub(crate) const MIN_ASSET_BYTES: u64 = 48 + 24 + 16 + 1;
pub(crate) const MIN_SAMPLE_RATE: u32 = 8_000;
pub(crate) const MAX_SAMPLE_RATE: u32 = 192_000;
pub(crate) const MAX_CHANNELS: u8 = 8;

/// Stable, path-safe identifier for one encrypted audio asset.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AssetId(Uuid);

impl AssetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for AssetId {
    type Err = VaultError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| VaultError::InvalidAssetId)
    }
}

/// Secret supplied by the application and zeroized when dropped.
pub struct MasterSecret(Zeroizing<[u8; 32]>);

impl MasterSecret {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for MasterSecret {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Debug for MasterSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterSecret(REDACTED)")
    }
}

impl Drop for MasterSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Audio encoding carried by a vault asset.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    PcmS16Le,
    Opus,
    Aac,
}

impl AudioCodec {
    pub(crate) const fn to_byte(self) -> u8 {
        match self {
            Self::PcmS16Le => 1,
            Self::Opus => 2,
            Self::Aac => 3,
        }
    }

    pub(crate) const fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::PcmS16Le),
            2 => Some(Self::Opus),
            3 => Some(Self::Aac),
            _ => None,
        }
    }
}

/// Validated format metadata shared by all chunks in one asset.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioAssetMetadata {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
}

impl AudioAssetMetadata {
    pub fn new(codec: AudioCodec, sample_rate: u32, channels: u8) -> Result<Self, VaultError> {
        let metadata = Self {
            codec,
            sample_rate,
            channels,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    pub(crate) fn validate(&self) -> Result<(), VaultError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.sample_rate) {
            return Err(VaultError::InvalidAudioMetadata(
                "sample rate must be between 8 kHz and 192 kHz",
            ));
        }
        if !(1..=MAX_CHANNELS).contains(&self.channels) {
            return Err(VaultError::InvalidAudioMetadata(
                "channel count must be between 1 and 8",
            ));
        }
        Ok(())
    }
}

/// Bounded metadata attached to one encrypted audio chunk.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioChunkMetadata {
    pub sequence: u32,
    pub timestamp_micros: u64,
    pub duration_micros: u32,
}

impl AudioChunkMetadata {
    pub fn new(
        sequence: u32,
        timestamp_micros: u64,
        duration_micros: u32,
    ) -> Result<Self, VaultError> {
        let metadata = Self {
            sequence,
            timestamp_micros,
            duration_micros,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    pub(crate) fn validate(&self) -> Result<(), VaultError> {
        if self.duration_micros == 0 || self.duration_micros > MAX_CHUNK_DURATION_MICROS {
            return Err(VaultError::InvalidAudioMetadata(
                "chunk duration must be between 1 microsecond and 60 seconds",
            ));
        }
        Ok(())
    }
}

/// A validated audio-only payload. The configured vault may impose a tighter bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioChunk {
    pub metadata: AudioChunkMetadata,
    pub payload: Vec<u8>,
}

impl AudioChunk {
    pub fn new(metadata: AudioChunkMetadata, payload: Vec<u8>) -> Result<Self, VaultError> {
        metadata.validate()?;
        if payload.is_empty() {
            return Err(VaultError::EmptyChunk);
        }
        if payload.len() > MAX_CHUNK_BYTES as usize {
            return Err(VaultError::ChunkTooLarge {
                actual: payload.len(),
                maximum: MAX_CHUNK_BYTES,
            });
        }
        Ok(Self { metadata, payload })
    }
}

/// Validated limits for a vault and its individual assets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultLimits {
    pub quota_bytes: u64,
    pub max_asset_bytes: u64,
    pub max_chunk_bytes: u32,
    pub max_chunks_per_asset: u32,
}

impl VaultLimits {
    pub fn new(
        quota_bytes: u64,
        max_asset_bytes: u64,
        max_chunk_bytes: u32,
        max_chunks_per_asset: u32,
    ) -> Result<Self, VaultError> {
        if quota_bytes == 0 || quota_bytes > MAX_VAULT_BYTES {
            return Err(VaultError::InvalidConfig(
                "quota must be between 1 byte and 1 TiB".to_owned(),
            ));
        }
        if !(MIN_ASSET_BYTES..=MAX_ASSET_BYTES).contains(&max_asset_bytes) {
            return Err(VaultError::InvalidConfig(
                "maximum asset size is outside the supported bounds".to_owned(),
            ));
        }
        if max_asset_bytes > quota_bytes {
            return Err(VaultError::InvalidConfig(
                "maximum asset size cannot exceed the quota".to_owned(),
            ));
        }
        if max_chunk_bytes == 0 || max_chunk_bytes > MAX_CHUNK_BYTES {
            return Err(VaultError::InvalidConfig(
                "maximum chunk size is outside the supported bound".to_owned(),
            ));
        }
        if max_chunks_per_asset == 0 || max_chunks_per_asset > MAX_CHUNKS_PER_ASSET {
            return Err(VaultError::InvalidConfig(
                "maximum chunk count is outside the supported bound".to_owned(),
            ));
        }
        Ok(Self {
            quota_bytes,
            max_asset_bytes,
            max_chunk_bytes,
            max_chunks_per_asset,
        })
    }
}

impl Default for VaultLimits {
    fn default() -> Self {
        Self {
            quota_bytes: 512 * 1024 * 1024,
            max_asset_bytes: 256 * 1024 * 1024,
            max_chunk_bytes: 4 * 1024 * 1024,
            max_chunks_per_asset: 100_000,
        }
    }
}

/// Configuration for a vault. The master secret is never included in debug output.
pub struct VaultConfig {
    pub root: PathBuf,
    pub limits: VaultLimits,
    pub(crate) master_secret: MasterSecret,
}

impl VaultConfig {
    pub fn new(
        root: impl Into<PathBuf>,
        master_secret: impl Into<MasterSecret>,
    ) -> Result<Self, VaultError> {
        let config = Self {
            root: root.into(),
            limits: VaultLimits::default(),
            master_secret: master_secret.into(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn with_limits(mut self, limits: VaultLimits) -> Result<Self, VaultError> {
        limits.validate()?;
        self.limits = limits;
        Ok(self)
    }

    pub(crate) fn validate(&self) -> Result<(), VaultError> {
        if self.root.as_os_str().is_empty() {
            return Err(VaultError::InvalidConfig(
                "vault root must not be empty".to_owned(),
            ));
        }
        self.limits.validate()
    }
}

impl VaultLimits {
    fn validate(&self) -> Result<(), VaultError> {
        Self::new(
            self.quota_bytes,
            self.max_asset_bytes,
            self.max_chunk_bytes,
            self.max_chunks_per_asset,
        )
        .map(|_| ())
    }
}

impl fmt::Debug for VaultConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultConfig")
            .field("root", &self.root)
            .field("limits", &self.limits)
            .field("master_secret", &"REDACTED")
            .finish()
    }
}
