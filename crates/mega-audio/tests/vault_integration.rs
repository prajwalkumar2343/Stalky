use std::{fs, str::FromStr};

use mega_audio::{
    AssetId, AudioAssetMetadata, AudioChunk, AudioChunkMetadata, AudioCodec, Vault, VaultConfig,
    VaultError, VaultLimits,
};
use tempfile::TempDir;

fn vault() -> (TempDir, Vault) {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let config = VaultConfig::new(temp_dir.path(), [7u8; 32]).expect("valid config");
    (temp_dir, Vault::new(config).expect("vault"))
}

fn metadata() -> AudioAssetMetadata {
    AudioAssetMetadata::new(AudioCodec::Opus, 48_000, 2).expect("format")
}

fn chunk(sequence: u32, payload: &[u8]) -> AudioChunk {
    AudioChunk::new(
        AudioChunkMetadata::new(sequence, u64::from(sequence) * 20_000, 20_000)
            .expect("chunk metadata"),
        payload.to_vec(),
    )
    .expect("chunk")
}

#[test]
fn roundtrip_streams_multiple_authenticated_audio_chunks() {
    let (_temp_dir, vault) = vault();
    let mut writer = vault.begin_asset(metadata()).expect("writer");
    let asset_id = writer.asset_id();
    writer
        .write_chunk(chunk(0, b"first audio frame"))
        .expect("first chunk");
    writer
        .write_chunk(chunk(1, b"second audio frame"))
        .expect("second chunk");
    assert_eq!(writer.finish().expect("finish"), asset_id);

    let mut reader = vault.open(asset_id).expect("open");
    assert_eq!(reader.metadata(), metadata());
    assert_eq!(
        reader.next_chunk().expect("read first"),
        Some(chunk(0, b"first audio frame"))
    );
    assert_eq!(
        reader.next_chunk().expect("read second"),
        Some(chunk(1, b"second audio frame"))
    );
    assert_eq!(reader.next_chunk().expect("read eof"), None);
    assert_eq!(vault.usage().expect("usage").asset_count, 1);
}

#[test]
fn tampering_with_ciphertext_fails_authentication() {
    let (_temp_dir, vault) = vault();
    let mut writer = vault.begin_asset(metadata()).expect("writer");
    let asset_id = writer.asset_id();
    writer
        .write_chunk(chunk(0, b"secret audio"))
        .expect("chunk");
    writer.finish().expect("finish");

    let path = vault.asset_path(asset_id);
    let mut bytes = fs::read(&path).expect("read asset");
    let tamper_offset = 48 + 24 + 2;
    bytes[tamper_offset] ^= 0x80;
    fs::write(path, bytes).expect("tamper asset");

    let mut reader = vault.open(asset_id).expect("open tampered asset");
    assert!(matches!(
        reader.next_chunk(),
        Err(VaultError::AuthenticationFailed)
    ));
}

#[test]
fn traversal_and_symlink_paths_are_rejected() {
    let (_temp_dir, vault) = vault();
    assert!(matches!(
        AssetId::from_str("../../outside"),
        Err(VaultError::InvalidAssetId)
    ));

    let outside = tempfile::NamedTempFile::new().expect("outside file");
    let id = AssetId::new();
    let target = vault.asset_path(id);
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &target).expect("symlink");
    #[cfg(unix)]
    assert!(matches!(vault.open(id), Err(VaultError::UnsafePath { .. })));
}

#[test]
fn dropping_interrupted_writer_removes_temp_and_never_commits_partial_asset() {
    let (_temp_dir, vault) = vault();
    let asset_id = {
        let mut writer = vault.begin_asset(metadata()).expect("writer");
        let asset_id = writer.asset_id();
        writer
            .write_chunk(chunk(0, b"partial audio"))
            .expect("chunk");
        asset_id
    };

    assert!(!vault.asset_path(asset_id).exists());
    let entries: Vec<_> = fs::read_dir(vault.temp_dir())
        .expect("temp entries")
        .collect();
    assert!(entries.is_empty());
}

#[test]
fn startup_cleanup_removes_orphaned_owned_temps_and_preserves_unknown_files() {
    let (temp_dir, vault) = vault();
    let orphan = vault.temp_dir().join(".stalky-audio-orphan.tmp");
    fs::write(&orphan, b"partial encrypted file").expect("orphan");
    let unknown = vault.temp_dir().join("keep-me.txt");
    fs::write(&unknown, b"unrelated").expect("unknown");
    drop(vault);

    let config = VaultConfig::new(temp_dir.path(), [7u8; 32]).expect("config");
    let vault = Vault::new(config).expect("restart vault");
    assert!(!orphan.exists());
    assert!(unknown.exists());
    assert_eq!(vault.cleanup_startup().expect("cleanup").removed_files, 0);
}

#[test]
fn delete_removes_asset_and_releases_accounted_storage() {
    let (_temp_dir, vault) = vault();
    let mut writer = vault.begin_asset(metadata()).expect("writer");
    let asset_id = writer.asset_id();
    writer.write_chunk(chunk(0, b"delete me")).expect("chunk");
    writer.finish().expect("finish");
    assert_eq!(vault.usage().expect("usage").asset_count, 1);

    assert!(vault.delete(asset_id).expect("delete"));
    assert!(!vault.asset_path(asset_id).exists());
    assert_eq!(vault.usage().expect("usage").asset_count, 0);
    assert!(!vault.delete(asset_id).expect("missing delete"));
}

#[test]
fn quota_and_chunk_contracts_are_enforced_before_persistence() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let limits = VaultLimits::new(150, 140, 32, 2).expect("limits");
    let config = VaultConfig::new(temp_dir.path(), [3u8; 32])
        .expect("config")
        .with_limits(limits)
        .expect("limited config");
    let vault = Vault::new(config).expect("vault");
    let mut writer = vault.begin_asset(metadata()).expect("writer");
    writer
        .write_chunk(chunk(0, &[9u8; 32]))
        .expect("first asset chunk");
    writer.finish().expect("first asset");
    let error = match vault.begin_asset(metadata()) {
        Ok(_) => panic!("quota should reject a second asset"),
        Err(error) => error,
    };
    assert!(matches!(error, VaultError::QuotaExceeded { .. }));
}
