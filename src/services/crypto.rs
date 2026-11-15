use aes_gcm::aead::{Aead, Generate, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("CREDENTIALS_KEY must be 32 bytes, base64 encoded")]
    InvalidKey,
    #[error("stored secret is malformed or was encrypted with a different key")]
    Decrypt,
}

#[derive(Clone)]
pub struct Cipher(Aes256Gcm);

impl Cipher {
    pub fn from_base64_key(key: &str) -> Result<Self, CryptoError> {
        let bytes = STANDARD
            .decode(key.trim())
            .map_err(|_| CryptoError::InvalidKey)?;
        Aes256Gcm::new_from_slice(&bytes)
            .map(Self)
            .map_err(|_| CryptoError::InvalidKey)
    }

    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(PREFIX)
    }

    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = Nonce::generate();
        let ciphertext = self
            .0
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("AES-GCM encryption of an in-memory string cannot fail");
        let mut raw = nonce.to_vec();
        raw.extend_from_slice(&ciphertext);
        format!("{PREFIX}{}", STANDARD.encode(raw))
    }

    /// Values without the version prefix are legacy plaintext and are returned unchanged.
    pub fn decrypt(&self, value: &str) -> Result<String, CryptoError> {
        let Some(encoded) = value.strip_prefix(PREFIX) else {
            return Ok(value.to_owned());
        };
        let raw = STANDARD.decode(encoded).map_err(|_| CryptoError::Decrypt)?;
        if raw.len() < NONCE_LEN {
            return Err(CryptoError::Decrypt);
        }
        let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce).map_err(|_| CryptoError::Decrypt)?;
        let plaintext = self
            .0
            .decrypt(&nonce, ciphertext)
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plaintext).map_err(|_| CryptoError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

    #[test]
    fn round_trip_and_legacy_plaintext() {
        let cipher = Cipher::from_base64_key(KEY).unwrap();
        let sealed = cipher.encrypt("s3cr3t");
        assert!(Cipher::is_encrypted(&sealed));
        assert_ne!(sealed, cipher.encrypt("s3cr3t"));
        assert_eq!(cipher.decrypt(&sealed).unwrap(), "s3cr3t");
        assert_eq!(cipher.decrypt("plain").unwrap(), "plain");
    }

    #[test]
    fn rejects_bad_key_and_tampering() {
        assert!(Cipher::from_base64_key("c2hvcnQ=").is_err());
        let cipher = Cipher::from_base64_key(KEY).unwrap();
        let other = Cipher::from_base64_key(&STANDARD.encode([7u8; 32])).unwrap();
        let sealed = cipher.encrypt("s3cr3t");
        assert!(other.decrypt(&sealed).is_err());
        assert!(cipher.decrypt("v1:AAAA").is_err());
    }
}
