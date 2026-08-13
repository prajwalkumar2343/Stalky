use std::{
    fs::File,
    io::{self, Read},
};

use crate::{
    AudioCodec, VaultError,
    contract::{AssetId, AudioAssetMetadata, AudioChunkMetadata},
    crypto::{NONCE_BYTES, TAG_BYTES},
};

pub(crate) const HEADER_LEN: usize = 48;
pub(crate) const RECORD_HEADER_LEN: usize = 24;
pub(crate) const FILE_EXTENSION: &str = "aud";
pub(crate) const TEMP_PREFIX: &str = ".stalky-audio-";
pub(crate) const TEMP_EXTENSION: &str = "tmp";
pub(crate) const ASSETS_DIR: &str = "assets";
pub(crate) const TEMP_DIR: &str = "tmp";

const MAGIC: &[u8; 8] = b"STALKYAV";
const FORMAT_VERSION: u8 = 1;

pub(crate) fn is_owned_temp_path(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(TEMP_PREFIX)
                && path.extension().and_then(|ext| ext.to_str()) == Some(TEMP_EXTENSION)
        })
}

pub(crate) fn random_nonce() -> [u8; NONCE_BYTES] {
    let uuid = AssetId::new().as_uuid();
    let mut nonce = [0u8; NONCE_BYTES];
    nonce.copy_from_slice(&uuid.as_bytes()[..NONCE_BYTES]);
    nonce
}

pub(crate) fn encode_header(
    asset_id: AssetId,
    nonce: [u8; NONCE_BYTES],
    metadata: AudioAssetMetadata,
) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(FORMAT_VERSION);
    header.extend_from_slice(&(HEADER_LEN as u16).to_be_bytes());
    header.extend_from_slice(asset_id.as_uuid().as_bytes());
    header.extend_from_slice(&nonce);
    header.push(metadata.codec.to_byte());
    header.extend_from_slice(&metadata.sample_rate.to_be_bytes());
    header.push(metadata.channels);
    header.extend_from_slice(&[0u8; 3]);
    debug_assert_eq!(header.len(), HEADER_LEN);
    header
}

pub(crate) fn read_header(
    file: &File,
    expected_asset_id: AssetId,
) -> Result<(Vec<u8>, AudioAssetMetadata, [u8; NONCE_BYTES]), VaultError> {
    let mut header = vec![0u8; HEADER_LEN];
    let mut reader = file;
    reader
        .read_exact(&mut header)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => VaultError::Corrupt("truncated audio vault header"),
            _ => VaultError::Io(error),
        })?;
    if &header[..MAGIC.len()] != MAGIC || header[8] != FORMAT_VERSION {
        return Err(VaultError::Corrupt("unknown audio vault header"));
    }
    let header_len = u16::from_be_bytes(header[9..11].try_into().unwrap()) as usize;
    if header_len != HEADER_LEN {
        return Err(VaultError::Corrupt("unsupported audio vault header length"));
    }
    let header_asset_id = AssetId::from_uuid(
        uuid::Uuid::from_slice(&header[11..27])
            .map_err(|_| VaultError::Corrupt("invalid asset id bytes"))?,
    );
    if header_asset_id != expected_asset_id {
        return Err(VaultError::Corrupt("asset id does not match its path"));
    }
    let nonce: [u8; NONCE_BYTES] = header[27..39]
        .try_into()
        .map_err(|_| VaultError::Corrupt("invalid asset nonce"))?;
    let codec =
        AudioCodec::from_byte(header[39]).ok_or(VaultError::Corrupt("unknown audio codec"))?;
    let sample_rate = u32::from_be_bytes(header[40..44].try_into().unwrap());
    let channels = header[44];
    let metadata = AudioAssetMetadata::new(codec, sample_rate, channels)
        .map_err(|_| VaultError::Corrupt("invalid audio format metadata"))?;
    Ok((header, metadata, nonce))
}

pub(crate) fn encode_record_header(
    metadata: &AudioChunkMetadata,
    plaintext_len: usize,
) -> [u8; RECORD_HEADER_LEN] {
    let mut record = [0u8; RECORD_HEADER_LEN];
    record[..4].copy_from_slice(&metadata.sequence.to_be_bytes());
    record[4..12].copy_from_slice(&metadata.timestamp_micros.to_be_bytes());
    record[12..16].copy_from_slice(&metadata.duration_micros.to_be_bytes());
    record[16..20].copy_from_slice(&(plaintext_len as u32).to_be_bytes());
    record[20..24].copy_from_slice(&((plaintext_len + TAG_BYTES) as u32).to_be_bytes());
    record
}

pub(crate) fn decode_record_metadata(
    record: &[u8; RECORD_HEADER_LEN],
) -> Result<AudioChunkMetadata, VaultError> {
    let sequence = u32::from_be_bytes(record[..4].try_into().unwrap());
    let timestamp_micros = u64::from_be_bytes(record[4..12].try_into().unwrap());
    let duration_micros = u32::from_be_bytes(record[12..16].try_into().unwrap());
    AudioChunkMetadata::new(sequence, timestamp_micros, duration_micros)
        .map_err(|_| VaultError::Corrupt("invalid chunk metadata"))
}

pub(crate) fn associated_data(header: &[u8], record_header: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + record_header.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(record_header);
    aad
}
