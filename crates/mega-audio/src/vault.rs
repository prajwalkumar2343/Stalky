use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{
    VaultError,
    contract::{AssetId, AudioAssetMetadata, AudioChunk, MasterSecret, VaultConfig, VaultLimits},
    crypto::{AssetCipher, TAG_BYTES},
    format::{
        ASSETS_DIR, FILE_EXTENSION, HEADER_LEN, RECORD_HEADER_LEN, TEMP_DIR, TEMP_EXTENSION,
        TEMP_PREFIX, associated_data, decode_record_metadata, encode_header, encode_record_header,
        is_owned_temp_path, random_nonce, read_header,
    },
};

#[derive(Default)]
struct QuotaState {
    reserved_bytes: u64,
}

/// Filesystem-backed encrypted audio vault.
pub struct Vault {
    root: PathBuf,
    assets_dir: PathBuf,
    temp_dir: PathBuf,
    limits: VaultLimits,
    master_secret: MasterSecret,
    quota_state: Arc<Mutex<QuotaState>>,
}

impl Vault {
    /// Creates the canonical vault directories and removes stale owned temp files.
    pub fn new(config: VaultConfig) -> Result<Self, VaultError> {
        config.validate()?;
        fs::create_dir_all(&config.root)?;
        if !fs::metadata(&config.root)?.is_dir() {
            return Err(VaultError::InvalidConfig(
                "vault root must be a directory".to_owned(),
            ));
        }
        let root = fs::canonicalize(&config.root)?;
        let assets_dir = root.join(ASSETS_DIR);
        let temp_dir = root.join(TEMP_DIR);
        fs::create_dir_all(&assets_dir)?;
        fs::create_dir_all(&temp_dir)?;
        let assets_dir = fs::canonicalize(assets_dir)?;
        let temp_dir = fs::canonicalize(temp_dir)?;
        if assets_dir.parent() != Some(root.as_path()) || temp_dir.parent() != Some(root.as_path())
        {
            return Err(VaultError::UnsafePath { path: root });
        }
        let vault = Self {
            root,
            assets_dir,
            temp_dir,
            limits: config.limits,
            master_secret: config.master_secret,
            quota_state: Arc::new(Mutex::new(QuotaState::default())),
        };
        vault.cleanup_startup()?;
        Ok(vault)
    }

    pub fn limits(&self) -> VaultLimits {
        self.limits
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the owned temp directory for diagnostics and startup cleanup tests.
    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    /// Returns the canonical final path for an asset without touching the filesystem.
    pub fn asset_path(&self, asset_id: AssetId) -> PathBuf {
        self.assets_dir.join(format!("{asset_id}.{FILE_EXTENSION}"))
    }

    /// Removes stale temp files left by interrupted writers and returns reclaimed bytes.
    pub fn cleanup_startup(&self) -> Result<CleanupReport, VaultError> {
        let mut report = CleanupReport::default();
        for directory in [&self.temp_dir, &self.assets_dir] {
            let entries = fs::read_dir(directory)?;
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if !is_owned_temp_path(&path) {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_dir() {
                    continue;
                }
                fs::remove_file(&path)?;
                report.removed_files = report.removed_files.saturating_add(1);
                report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(metadata.len());
            }
        }
        Ok(report)
    }

    /// Returns committed audio-file usage. Temp reservations are intentionally excluded.
    pub fn usage(&self) -> Result<VaultUsage, VaultError> {
        let _guard = self
            .quota_state
            .lock()
            .map_err(|_| VaultError::Corrupt("vault quota state was poisoned"))?;
        self.scan_usage()
    }

    /// Starts a new asset. The returned writer commits only after `finish` succeeds.
    pub fn begin_asset(&self, metadata: AudioAssetMetadata) -> Result<AssetWriter, VaultError> {
        metadata.validate()?;
        let asset_id = AssetId::new();
        let asset_path = self.asset_path(asset_id);
        if asset_path.exists() {
            return Err(VaultError::AlreadyExists);
        }

        let nonce = random_nonce();
        let header = encode_header(asset_id, nonce, metadata);
        let cipher = AssetCipher::new(self.master_secret.as_bytes(), asset_id, nonce)?;
        let temp_path = self
            .temp_dir
            .join(format!("{TEMP_PREFIX}{asset_id}.{TEMP_EXTENSION}"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        let header_size = header.len() as u64;
        let reservation = match (|| {
            let mut state = self
                .quota_state
                .lock()
                .map_err(|_| VaultError::Corrupt("vault quota state was poisoned"))?;
            let usage = self.scan_usage()?;
            reserve_bytes(&state, usage.bytes, header_size, self.limits)?;
            state.reserved_bytes = state.reserved_bytes.saturating_add(header_size);
            Ok::<u64, VaultError>(header_size)
        })() {
            Ok(reservation) => reservation,
            Err(error) => {
                drop(file);
                let _ = fs::remove_file(&temp_path);
                return Err(error);
            }
        };

        let mut file = file;
        if let Err(error) = file.write_all(&header).and_then(|_| file.flush()) {
            release_reservation(&self.quota_state, reservation);
            let _ = fs::remove_file(&temp_path);
            return Err(error.into());
        }

        Ok(AssetWriter {
            file: Some(file),
            temp_path,
            asset_path,
            asset_id,
            header,
            cipher,
            next_sequence: 0,
            current_size: header_size,
            chunk_count: 0,
            quota_state: Arc::clone(&self.quota_state),
            limits: self.limits,
            reservation_bytes: reservation,
            finished: false,
        })
    }

    /// Opens a committed asset for authenticated, bounded chunk-by-chunk decryption.
    pub fn open(&self, asset_id: AssetId) -> Result<AssetReader, VaultError> {
        let path = self.existing_asset_path(asset_id)?;
        let file = open_asset_file(&path)?;
        let file_len = file.metadata()?.len();
        if file_len > self.limits.max_asset_bytes {
            return Err(VaultError::AssetTooLarge {
                requested: file_len,
                maximum: self.limits.max_asset_bytes,
            });
        }
        let (header, metadata, nonce) = read_header(&file, asset_id)?;
        let cipher = AssetCipher::new(self.master_secret.as_bytes(), asset_id, nonce)?;
        Ok(AssetReader {
            file,
            header,
            metadata,
            cipher,
            next_sequence: 0,
            chunk_count: 0,
            total_size: HEADER_LEN as u64,
            max_chunk_bytes: self.limits.max_chunk_bytes,
            max_chunks_per_asset: self.limits.max_chunks_per_asset,
            max_asset_bytes: self.limits.max_asset_bytes,
        })
    }

    /// Deletes one committed audio asset. Returns false when it did not exist.
    pub fn delete(&self, asset_id: AssetId) -> Result<bool, VaultError> {
        let _guard = self
            .quota_state
            .lock()
            .map_err(|_| VaultError::Corrupt("vault quota state was poisoned"))?;
        let path = self.asset_path(asset_id);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            return Err(VaultError::UnsafePath { path });
        }
        self.ensure_canonical_asset(&path)?;
        fs::remove_file(&path)?;
        sync_directory(&self.assets_dir)?;
        Ok(true)
    }

    fn existing_asset_path(&self, asset_id: AssetId) -> Result<PathBuf, VaultError> {
        let path = self.asset_path(asset_id);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(VaultError::UnsafePath { path })
            }
            Ok(_) => {
                self.ensure_canonical_asset(&path)?;
                Ok(path)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(VaultError::NotFound),
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_canonical_asset(&self, path: &Path) -> Result<(), VaultError> {
        let canonical = fs::canonicalize(path)?;
        if canonical.parent() != Some(self.assets_dir.as_path()) {
            return Err(VaultError::UnsafePath {
                path: path.to_owned(),
            });
        }
        Ok(())
    }

    fn scan_usage(&self) -> Result<VaultUsage, VaultError> {
        let mut usage = VaultUsage::default();
        for entry in fs::read_dir(&self.assets_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(FILE_EXTENSION) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(VaultError::UnsafePath { path });
            }
            self.ensure_canonical_asset(&path)?;
            usage.asset_count = usage.asset_count.saturating_add(1);
            usage.bytes = usage.bytes.saturating_add(metadata.len());
        }
        Ok(usage)
    }
}

/// Summary of committed audio usage in a vault.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VaultUsage {
    pub asset_count: u64,
    pub bytes: u64,
}

/// Number and size of temp files removed during startup cleanup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CleanupReport {
    pub removed_files: u64,
    pub reclaimed_bytes: u64,
}

/// Atomic streaming writer for one encrypted audio asset.
pub struct AssetWriter {
    file: Option<File>,
    temp_path: PathBuf,
    asset_path: PathBuf,
    asset_id: AssetId,
    header: Vec<u8>,
    cipher: AssetCipher,
    next_sequence: u32,
    current_size: u64,
    chunk_count: u32,
    quota_state: Arc<Mutex<QuotaState>>,
    limits: VaultLimits,
    reservation_bytes: u64,
    finished: bool,
}

impl AssetWriter {
    pub fn asset_id(&self) -> AssetId {
        self.asset_id
    }

    pub fn write_chunk(&mut self, chunk: AudioChunk) -> Result<(), VaultError> {
        chunk.metadata.validate()?;
        if chunk.payload.is_empty() {
            return Err(VaultError::EmptyChunk);
        }
        if chunk.metadata.sequence != self.next_sequence {
            return Err(VaultError::InvalidAudioMetadata(
                "chunk sequence must increase from zero without gaps",
            ));
        }
        if chunk.payload.len() > self.limits.max_chunk_bytes as usize {
            return Err(VaultError::ChunkTooLarge {
                actual: chunk.payload.len(),
                maximum: self.limits.max_chunk_bytes,
            });
        }
        if self.chunk_count >= self.limits.max_chunks_per_asset {
            return Err(VaultError::ChunkCountExceeded);
        }

        let record_header = encode_record_header(&chunk.metadata, chunk.payload.len());
        let encrypted = self.cipher.encrypt(
            self.next_sequence,
            &chunk.payload,
            &associated_data(&self.header, &record_header),
        )?;
        let record_size = RECORD_HEADER_LEN as u64 + encrypted.len() as u64;
        let next_size =
            self.current_size
                .checked_add(record_size)
                .ok_or(VaultError::AssetTooLarge {
                    requested: u64::MAX,
                    maximum: self.limits.max_asset_bytes,
                })?;
        if next_size > self.limits.max_asset_bytes {
            return Err(VaultError::AssetTooLarge {
                requested: next_size,
                maximum: self.limits.max_asset_bytes,
            });
        }

        let additional_reservation = next_size - self.reservation_bytes;
        {
            let mut state = self
                .quota_state
                .lock()
                .map_err(|_| VaultError::Corrupt("vault quota state was poisoned"))?;
            let usage = scan_usage_from_assets(&self.asset_path)?;
            reserve_bytes(&state, usage.bytes, additional_reservation, self.limits)?;
            state.reserved_bytes = state.reserved_bytes.saturating_add(additional_reservation);
        }

        let write_result = self
            .file
            .as_mut()
            .ok_or(VaultError::Corrupt("asset writer was already finished"))
            .and_then(|file| {
                file.write_all(&record_header)?;
                file.write_all(&encrypted)?;
                file.flush()?;
                Ok(())
            });
        if let Err(error) = write_result {
            release_reservation(&self.quota_state, additional_reservation);
            return Err(error);
        }

        self.reservation_bytes = next_size;
        self.current_size = next_size;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.chunk_count = self.chunk_count.saturating_add(1);
        Ok(())
    }

    /// Flushes, syncs, and atomically renames the temp file into the asset directory.
    pub fn finish(mut self) -> Result<AssetId, VaultError> {
        if self.chunk_count == 0 {
            return Err(VaultError::Corrupt(
                "an audio asset must contain at least one chunk",
            ));
        }
        let file = self
            .file
            .as_mut()
            .ok_or(VaultError::Corrupt("asset writer was already finished"))?;
        file.sync_all()?;

        let mut state = self
            .quota_state
            .lock()
            .map_err(|_| VaultError::Corrupt("vault quota state was poisoned"))?;
        if self.asset_path.exists() {
            return Err(VaultError::AlreadyExists);
        }
        fs::rename(&self.temp_path, &self.asset_path)?;
        sync_directory(
            self.asset_path
                .parent()
                .ok_or(VaultError::Corrupt("asset path has no parent"))?,
        )?;
        state.reserved_bytes = state.reserved_bytes.saturating_sub(self.reservation_bytes);
        self.reservation_bytes = 0;
        self.finished = true;
        self.file.take();
        Ok(self.asset_id)
    }
}

impl Drop for AssetWriter {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.file.take();
        let _ = fs::remove_file(&self.temp_path);
        release_reservation(&self.quota_state, self.reservation_bytes);
        self.reservation_bytes = 0;
    }
}

/// Authenticated streaming reader for a committed asset.
pub struct AssetReader {
    file: File,
    header: Vec<u8>,
    metadata: AudioAssetMetadata,
    cipher: AssetCipher,
    next_sequence: u32,
    chunk_count: u32,
    total_size: u64,
    max_chunk_bytes: u32,
    max_chunks_per_asset: u32,
    max_asset_bytes: u64,
}

impl AssetReader {
    pub fn metadata(&self) -> AudioAssetMetadata {
        self.metadata
    }

    pub fn next_chunk(&mut self) -> Result<Option<AudioChunk>, VaultError> {
        let mut record_header = [0u8; RECORD_HEADER_LEN];
        match self.file.read(&mut record_header[..1])? {
            0 if self.chunk_count == 0 => {
                return Err(VaultError::Corrupt("asset contains no complete chunks"));
            }
            0 => return Ok(None),
            1 => self
                .file
                .read_exact(&mut record_header[1..])
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::UnexpectedEof {
                        VaultError::Corrupt("truncated chunk record header")
                    } else {
                        VaultError::Io(error)
                    }
                })?,
            _ => unreachable!("a one-byte read cannot return more than one byte"),
        }

        if self.chunk_count >= self.max_chunks_per_asset {
            return Err(VaultError::ChunkCountExceeded);
        }
        let metadata = decode_record_metadata(&record_header)?;
        if metadata.sequence != self.next_sequence {
            return Err(VaultError::Corrupt("chunk sequence is out of order"));
        }
        let plaintext_len = u32::from_be_bytes(record_header[16..20].try_into().unwrap());
        let ciphertext_len = u32::from_be_bytes(record_header[20..24].try_into().unwrap());
        if plaintext_len == 0 || plaintext_len > self.max_chunk_bytes {
            return Err(VaultError::Corrupt(
                "chunk plaintext length is out of bounds",
            ));
        }
        let expected_ciphertext_len = plaintext_len
            .checked_add(TAG_BYTES as u32)
            .ok_or(VaultError::Corrupt("chunk ciphertext length overflow"))?;
        if ciphertext_len != expected_ciphertext_len {
            return Err(VaultError::Corrupt("chunk ciphertext length is invalid"));
        }
        let next_total_size = self
            .total_size
            .checked_add(RECORD_HEADER_LEN as u64 + u64::from(ciphertext_len))
            .ok_or(VaultError::Corrupt("asset size overflow"))?;
        if next_total_size > self.max_asset_bytes {
            return Err(VaultError::Corrupt("asset exceeds configured maximum size"));
        }

        let mut ciphertext = vec![0u8; ciphertext_len as usize];
        self.file
            .read_exact(&mut ciphertext)
            .map_err(|error| match error.kind() {
                io::ErrorKind::UnexpectedEof => VaultError::Corrupt("truncated encrypted chunk"),
                _ => VaultError::Io(error),
            })?;
        let plaintext = self.cipher.decrypt(
            self.next_sequence,
            &ciphertext,
            &associated_data(&self.header, &record_header),
        )?;
        if plaintext.len() != plaintext_len as usize {
            return Err(VaultError::Corrupt(
                "decrypted chunk length does not match metadata",
            ));
        }

        self.total_size = next_total_size;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.chunk_count = self.chunk_count.saturating_add(1);
        Ok(Some(AudioChunk {
            metadata,
            payload: plaintext,
        }))
    }
}

fn reserve_bytes(
    state: &QuotaState,
    committed_bytes: u64,
    additional_bytes: u64,
    limits: VaultLimits,
) -> Result<(), VaultError> {
    let requested = committed_bytes
        .checked_add(state.reserved_bytes)
        .and_then(|value| value.checked_add(additional_bytes))
        .ok_or(VaultError::QuotaExceeded {
            requested: u64::MAX,
            quota: limits.quota_bytes,
        })?;
    if requested > limits.quota_bytes {
        return Err(VaultError::QuotaExceeded {
            requested,
            quota: limits.quota_bytes,
        });
    }
    Ok(())
}

fn release_reservation(state: &Arc<Mutex<QuotaState>>, amount: u64) {
    if let Ok(mut state) = state.lock() {
        state.reserved_bytes = state.reserved_bytes.saturating_sub(amount);
    }
}

fn scan_usage_from_assets(asset_path: &Path) -> Result<VaultUsage, VaultError> {
    let assets_dir = asset_path
        .parent()
        .ok_or(VaultError::Corrupt("asset path has no parent"))?;
    let mut usage = VaultUsage::default();
    for entry in fs::read_dir(assets_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some(FILE_EXTENSION) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(VaultError::UnsafePath { path });
        }
        usage.asset_count = usage.asset_count.saturating_add(1);
        usage.bytes = usage.bytes.saturating_add(metadata.len());
    }
    Ok(usage)
}

fn sync_directory(path: &Path) -> Result<(), VaultError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn open_asset_file(path: &Path) -> Result<File, VaultError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) => {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ELOOP) {
                return Err(VaultError::UnsafePath {
                    path: path.to_owned(),
                });
            }
            Err(error.into())
        }
    }
}
