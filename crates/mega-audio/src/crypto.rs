use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::{VaultError, contract::AssetId};

pub(crate) const NONCE_BYTES: usize = 12;
pub(crate) const TAG_BYTES: usize = 16;

pub(crate) struct AssetCipher {
    cipher: Aes256Gcm,
    key: Zeroizing<[u8; 32]>,
    nonce: [u8; NONCE_BYTES],
}

impl AssetCipher {
    pub(crate) fn new(
        master_secret: &[u8; 32],
        asset_id: AssetId,
        nonce: [u8; NONCE_BYTES],
    ) -> Result<Self, VaultError> {
        let hkdf = Hkdf::<Sha256>::new(Some(&nonce), master_secret);
        let mut key = Zeroizing::new([0u8; 32]);
        let mut info = Vec::with_capacity(41);
        info.extend_from_slice(b"stalky/audio-vault/key/v1");
        info.extend_from_slice(asset_id.as_uuid().as_bytes());
        hkdf.expand(&info, &mut *key)
            .map_err(|_| VaultError::KeyDerivation)?;
        let cipher = Aes256Gcm::new_from_slice(&*key).map_err(|_| VaultError::KeyDerivation)?;
        Ok(Self { cipher, key, nonce })
    }

    pub(crate) fn encrypt(
        &self,
        sequence: u32,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        let nonce = self.chunk_nonce(sequence);
        self.cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)
    }

    pub(crate) fn decrypt(
        &self,
        sequence: u32,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        let nonce = self.chunk_nonce(sequence);
        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| VaultError::AuthenticationFailed)
    }

    fn chunk_nonce(&self, sequence: u32) -> [u8; NONCE_BYTES] {
        let mut nonce = self.nonce;
        let sequence_bytes = sequence.to_be_bytes();
        for (byte, sequence_byte) in nonce[8..].iter_mut().zip(sequence_bytes) {
            *byte ^= sequence_byte;
        }
        nonce
    }
}

impl Drop for AssetCipher {
    fn drop(&mut self) {
        // Keep the derived material explicitly zeroized even if the underlying
        // cipher implementation changes its drop behavior in a future release.
        self.key.zeroize();
    }
}
