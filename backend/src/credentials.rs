use std::fmt;

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

const PREFIX: &str = "v1:";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("provider credential encryption is not configured")]
    NotConfigured,
    #[error("provider credential encryption key is invalid")]
    InvalidKey,
    #[error("provider credential is empty")]
    Empty,
    #[error("provider credential ciphertext is invalid")]
    InvalidCiphertext,
}

/// A short-lived provider secret. Its debug representation is deliberately
/// redacted and its backing string is zeroized when dropped.
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct ProviderCredentialVault {
    key: [u8; 32],
}

impl fmt::Debug for ProviderCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderCredentialVault([REDACTED])")
    }
}

impl ProviderCredentialVault {
    pub fn from_env() -> Result<Self, CredentialError> {
        let value = std::env::var("STALKY_PROVIDER_CREDENTIAL_KEY")
            .map_err(|_| CredentialError::NotConfigured)?;
        Self::from_hex_key(&value)
    }

    pub fn from_hex_key(value: &str) -> Result<Self, CredentialError> {
        let bytes = hex::decode(value.trim()).map_err(|_| CredentialError::InvalidKey)?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| CredentialError::InvalidKey)?;
        Ok(Self { key })
    }

    pub fn seal(&self, plaintext: &str) -> Result<String, CredentialError> {
        if plaintext.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        let cipher =
            Aes256Gcm::new_from_slice(&self.key).map_err(|_| CredentialError::InvalidKey)?;
        let nonce = *Uuid::new_v4().as_bytes();
        let encrypted = cipher
            .encrypt(Nonce::from_slice(&nonce[..12]), plaintext.as_bytes())
            .map_err(|_| CredentialError::InvalidCiphertext)?;
        let mut payload = nonce[..12].to_vec();
        payload.extend(encrypted);
        Ok(format!("{PREFIX}{}", hex::encode(payload)))
    }

    pub fn open(&self, ciphertext: &str) -> Result<SecretString, CredentialError> {
        let encoded = ciphertext
            .strip_prefix(PREFIX)
            .ok_or(CredentialError::InvalidCiphertext)?;
        let payload = hex::decode(encoded).map_err(|_| CredentialError::InvalidCiphertext)?;
        if payload.len() <= 12 {
            return Err(CredentialError::InvalidCiphertext);
        }
        let cipher =
            Aes256Gcm::new_from_slice(&self.key).map_err(|_| CredentialError::InvalidKey)?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&payload[..12]), &payload[12..])
            .map_err(|_| CredentialError::InvalidCiphertext)?;
        let value = String::from_utf8(plaintext).map_err(|_| CredentialError::InvalidCiphertext)?;
        if value.trim().is_empty() {
            return Err(CredentialError::Empty);
        }
        Ok(SecretString::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_round_trip_without_exposing_plaintext_in_debug() {
        let vault = ProviderCredentialVault::from_hex_key(&"11".repeat(32)).unwrap();
        let ciphertext = vault.seal("provider-secret").unwrap();
        assert!(!ciphertext.contains("provider-secret"));
        let secret = vault.open(&ciphertext).unwrap();
        assert_eq!(secret.as_str(), "provider-secret");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
    }

    #[test]
    fn rejects_wrong_key_and_malformed_ciphertext() {
        let first = ProviderCredentialVault::from_hex_key(&"22".repeat(32)).unwrap();
        let second = ProviderCredentialVault::from_hex_key(&"33".repeat(32)).unwrap();
        let ciphertext = first.seal("secret").unwrap();
        assert!(matches!(
            second.open(&ciphertext),
            Err(CredentialError::InvalidCiphertext)
        ));
        assert!(matches!(
            first.open("not-a-secret"),
            Err(CredentialError::InvalidCiphertext)
        ));
    }
}
