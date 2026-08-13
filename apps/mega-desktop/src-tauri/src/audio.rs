use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use mega_audio::{
    AssetId, AudioAssetMetadata, AudioChunk, AudioChunkMetadata, AudioCodec, Vault, VaultConfig,
    VaultUsage,
};
use mega_audio_capture::{
    AudioSegment, AudioService, AudioServiceConfig, AudioSink, AudioSource, AudioStatus, SinkError,
};
use mega_memory::Sensitivity;
use mega_store::{
    AudioAssetInput, AudioAssetStatus, HistoryMediaKind, TimelineEntryInput, TimelineSourceKind,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::memory::MemoryService;

const KEYCHAIN_SERVICE: &str = "com.stalky.desktop.audio";
const KEYCHAIN_ACCOUNT: &str = "vault-key-v1";

pub struct AudioVaultService {
    root: Result<PathBuf, String>,
    vault: Arc<Mutex<Option<Result<Vault, String>>>>,
}

impl AudioVaultService {
    pub fn initialize(app: &AppHandle) -> Self {
        Self {
            root: app
                .path()
                .app_local_data_dir()
                .map(|directory| directory.join("audio-vault"))
                .map_err(|error| format!("could not resolve protected audio directory: {error}")),
            vault: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn with_vault<T>(
        &self,
        operation: impl FnOnce(&Vault) -> Result<T, mega_audio::VaultError>,
    ) -> Result<T, String> {
        let mut guard = self
            .vault
            .lock()
            .map_err(|_| "audio vault lock is unavailable".to_owned())?;
        let vault = guard.get_or_insert_with(|| self.root.clone().and_then(open_vault));
        let vault = vault.as_ref().map_err(Clone::clone)?;
        operation(vault).map_err(|error| error.to_string())
    }
}

impl Clone for AudioVaultService {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            vault: Arc::clone(&self.vault),
        }
    }
}

struct EncryptedHistorySink {
    vault: AudioVaultService,
    memory: MemoryService,
}

impl AudioSink for EncryptedHistorySink {
    fn store(&self, segment: AudioSegment) -> Result<(), SinkError> {
        let (metadata, mut samples) = segment.into_parts();
        let audio_metadata = AudioAssetMetadata::new(
            AudioCodec::PcmS16LeZstd,
            metadata.format.sample_rate_hz,
            u8::try_from(metadata.format.channels)
                .map_err(|_| SinkError::Rejected("unsupported channel count".into()))?,
        )
        .map_err(|error| SinkError::Rejected(error.to_string()))?;
        let plaintext = Zeroizing::new(
            samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        samples.zeroize();
        let payload = zstd::stream::encode_all(plaintext.as_slice(), 3)
            .map_err(|error| SinkError::Failed(format!("audio compression failed: {error}")))?;
        let duration_micros = u32::try_from(metadata.duration_nanos / 1_000)
            .map_err(|_| SinkError::Rejected("audio segment duration overflowed".into()))?;
        let timestamp_micros = metadata
            .timestamp
            .unix_millis
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_default()
            .saturating_mul(1_000);

        let (asset_id, storage_path, byte_size) = self
            .vault
            .with_vault(|vault| {
                let mut writer = vault.begin_asset(audio_metadata)?;
                let asset_id = writer.asset_id();
                writer.write_chunk(AudioChunk::new(
                    AudioChunkMetadata::new(0, timestamp_micros, duration_micros)?,
                    payload,
                )?)?;
                writer.finish()?;
                let storage_path = vault.asset_path(asset_id);
                let byte_size = std::fs::metadata(&storage_path)?.len();
                Ok((asset_id, storage_path, byte_size))
            })
            .map_err(SinkError::Failed)?;

        let started_at_ms = metadata.timestamp.unix_millis.unwrap_or_else(now_millis);
        let duration_ms = metadata.duration_nanos.saturating_add(999_999) / 1_000_000;
        let entry = TimelineEntryInput {
            id: Uuid::now_v7().to_string(),
            idempotency_key: format!(
                "audio:{}:{}:{}",
                metadata.provenance.generation, metadata.sequence, asset_id
            ),
            media_kind: HistoryMediaKind::Audio,
            source_kind: TimelineSourceKind::AudioCapture,
            bundle_identifier: None,
            app_display_name: Some(
                match metadata.provenance.source {
                    AudioSource::Microphone => "Microphone",
                    AudioSource::SystemAudio => "System Audio",
                }
                .to_owned(),
            ),
            redacted_window_title: None,
            started_at_ms,
            ended_at_ms: started_at_ms
                .saturating_add(i64::try_from(duration_ms).unwrap_or(i64::MAX)),
            text_content: None,
            capture_sequence: i64::try_from(metadata.sequence).ok(),
            ax_sequence: None,
            sensitivity: Sensitivity::Sensitive,
            created_at_ms: now_millis(),
            audio_asset: Some(AudioAssetInput {
                id: asset_id.to_string(),
                storage_path: Some(storage_path),
                object_key: None,
                byte_size,
                duration_ms,
                status: AudioAssetStatus::Ready,
            }),
        };
        if let Err(error) = self
            .memory
            .with_store(|store| store.admit_timeline_entry(&entry))
        {
            let _ = self.vault.with_vault(|vault| vault.delete(asset_id));
            return Err(SinkError::Failed(format!(
                "audio history metadata failed: {error:?}"
            )));
        }
        Ok(())
    }
}

pub struct AudioHistoryService {
    capture: AudioService,
}

impl AudioHistoryService {
    pub fn initialize(vault: AudioVaultService, memory: MemoryService) -> Result<Self, String> {
        let sink: Arc<dyn AudioSink> = Arc::new(EncryptedHistorySink { vault, memory });
        let capture = AudioService::new(
            mega_audio_capture::default_backend(),
            sink,
            AudioServiceConfig::default(),
        )
        .map_err(|error| error.to_string())?;
        Ok(Self { capture })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioVaultStatus {
    committed_bytes: u64,
    asset_count: u64,
    quota_bytes: u64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSourceRequest {
    Microphone,
    SystemAudio,
}

impl From<AudioSourceRequest> for AudioSource {
    fn from(value: AudioSourceRequest) -> Self {
        match value {
            AudioSourceRequest::Microphone => Self::Microphone,
            AudioSourceRequest::SystemAudio => Self::SystemAudio,
        }
    }
}

#[tauri::command]
pub fn audio_history_start(
    service: State<'_, AudioHistoryService>,
    source: AudioSourceRequest,
) -> Result<AudioStatus, String> {
    let source = source.into();
    let status = service.capture.status();
    if matches!(status.state, mega_audio_capture::AudioState::Running) {
        return Ok(status);
    }
    service
        .capture
        .start(source)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn audio_history_status(service: State<'_, AudioHistoryService>) -> AudioStatus {
    service.capture.status()
}

#[tauri::command]
pub fn audio_history_stop(service: State<'_, AudioHistoryService>) -> Result<AudioStatus, String> {
    service.capture.stop().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn audio_vault_status(
    service: State<'_, AudioVaultService>,
) -> Result<AudioVaultStatus, String> {
    let service = service.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.with_vault(|vault| {
            let VaultUsage {
                bytes: committed_bytes,
                asset_count,
            } = vault.usage()?;
            Ok(AudioVaultStatus {
                committed_bytes,
                asset_count,
                quota_bytes: vault.limits().quota_bytes,
            })
        })
    })
    .await
    .map_err(|error| format!("audio status worker failed: {error}"))?
}

pub(crate) fn delete_asset(service: &AudioVaultService, id: &str) -> Result<(), String> {
    let asset_id = id.parse::<AssetId>().map_err(|error| error.to_string())?;
    service.with_vault(|vault| vault.delete(asset_id).map(|_| ()))
}

#[cfg(target_os = "macos")]
fn open_vault(root: PathBuf) -> Result<Vault, String> {
    use getrandom::fill;
    use security_framework::passwords::{get_generic_password, set_generic_password};
    use zeroize::Zeroizing;

    let bytes = Zeroizing::new(
        match get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            Ok(bytes) => bytes,
            Err(error) if error.code() == -25300 => {
                let mut generated = vec![0_u8; 32];
                fill(&mut generated).map_err(|error| {
                    format!("could not generate an audio encryption key: {error}")
                })?;
                set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &generated).map_err(
                    |error| format!("could not store the audio key in Keychain: {error}"),
                )?;
                generated
            }
            Err(error) => {
                return Err(format!(
                    "could not read the audio key from Keychain: {error}"
                ));
            }
        },
    );
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "the Keychain audio key has an invalid length".to_owned())?;
    let config = VaultConfig::new(root, key).map_err(|error| error.to_string())?;
    Vault::new(config).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn open_vault(_root: PathBuf) -> Result<Vault, String> {
    Err("encrypted audio history is available only in the macOS desktop build".to_owned())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
