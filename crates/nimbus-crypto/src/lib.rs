//! Client-side encryption for Nimbus — zero-knowledge envelope encryption.
//!
//! ## Model
//! - A random 256-bit **data encryption key (DEK)** encrypts file contents.
//! - The DEK is *wrapped* (encrypted) by a **key-encryption key (KEK)** derived
//!   from the user's passphrase via Argon2id, and separately by a random
//!   **recovery key**. Either can unwrap the DEK.
//! - Files are encrypted with AES-256-GCM; a fresh random 96-bit nonce is
//!   prepended to each ciphertext. GCM's auth tag detects any tampering.
//!
//! The plaintext and the DEK never leave the process. GitHub only ever sees
//! ciphertext.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::rngs::OsRng;
use rand::RngCore;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed (wrong key or corrupt data)")]
    Decrypt,
    #[error("key derivation failed: {0}")]
    Derive(String),
    #[error("ciphertext too short")]
    Length,
    #[error("invalid recovery key")]
    RecoveryKey,
}

/// Compare two byte slices in (length-checked) constant time, to avoid leaking
/// secrets via timing. The length comparison itself is not constant-time, which
/// is acceptable for fixed-size hashes/tokens.
pub fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Fill an N-byte array with cryptographically secure random bytes.
fn random_array<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// Generate a random salt for passphrase derivation.
pub fn generate_salt() -> [u8; SALT_LEN] {
    random_array()
}

/// Generate a random 256-bit key (used for the DEK and recovery key).
pub fn generate_key() -> [u8; KEY_LEN] {
    random_array()
}

/// Derive a 256-bit KEK from a passphrase + salt using Argon2id.
pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], CryptoError> {
    let mut out = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| CryptoError::Derive(e.to_string()))?;
    Ok(out)
}

/// Encrypt `plaintext` with `key` and authenticated associated data `aad`.
/// Output is `nonce || ciphertext+tag`. The same `aad` must be supplied to
/// decrypt — binding e.g. a file path here prevents ciphertext substitution
/// across paths.
pub fn encrypt_aad(
    key: &[u8; KEY_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes = random_array::<NONCE_LEN>();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt data produced by [`encrypt_aad`] with the same `aad`.
pub fn decrypt_aad(key: &[u8; KEY_LEN], data: &[u8], aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::Length);
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

/// Encrypt with no associated data (used for key wrapping).
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    encrypt_aad(key, plaintext, &[])
}

/// Decrypt data produced by [`encrypt`].
pub fn decrypt(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    decrypt_aad(key, data, &[])
}

/// Wrap (encrypt) a DEK under a key-encryption key.
pub fn wrap_key(kek: &[u8; KEY_LEN], dek: &[u8; KEY_LEN]) -> Result<Vec<u8>, CryptoError> {
    encrypt(kek, dek)
}

/// Unwrap (decrypt) a DEK that was wrapped under `kek`.
pub fn unwrap_key(kek: &[u8; KEY_LEN], wrapped: &[u8]) -> Result<[u8; KEY_LEN], CryptoError> {
    let bytes = decrypt(kek, wrapped)?;
    bytes.try_into().map_err(|_| CryptoError::Decrypt)
}

/// Encode a recovery key as a human-transcribable base64 string.
pub fn encode_recovery_key(key: &[u8; KEY_LEN]) -> String {
    STANDARD.encode(key)
}

/// Decode a recovery key from its base64 string form.
pub fn decode_recovery_key(s: &str) -> Result<[u8; KEY_LEN], CryptoError> {
    let bytes = STANDARD
        .decode(s.trim())
        .map_err(|_| CryptoError::RecoveryKey)?;
    bytes.try_into().map_err(|_| CryptoError::RecoveryKey)
}

/// Holds an unlocked DEK in memory and seals/opens file bytes with it.
#[derive(Clone)]
pub struct Vault {
    dek: [u8; KEY_LEN],
}

impl Vault {
    /// Build a vault from an already-unwrapped DEK.
    pub fn new(dek: [u8; KEY_LEN]) -> Self {
        Self { dek }
    }

    /// Encrypt file bytes for storage, binding `context` (e.g. the file path)
    /// as associated data so the ciphertext cannot be replayed under another path.
    pub fn seal(&self, context: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        encrypt_aad(&self.dek, plaintext, context)
    }

    /// Decrypt file bytes read from storage; `context` must match what was sealed.
    pub fn open(&self, context: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        decrypt_aad(&self.dek, ciphertext, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_is_deterministic_per_salt() {
        let salt = [7u8; SALT_LEN];
        let a = derive_key("correct horse", &salt).unwrap();
        let b = derive_key("correct horse", &salt).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn derive_differs_by_salt_and_passphrase() {
        let k1 = derive_key("pw", &[1u8; SALT_LEN]).unwrap();
        let k2 = derive_key("pw", &[2u8; SALT_LEN]).unwrap();
        let k3 = derive_key("other", &[1u8; SALT_LEN]).unwrap();
        assert_ne!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = generate_key();
        let pt = b"top secret nimbus payload";
        let ct = encrypt(&key, pt).unwrap();
        assert_ne!(&ct[NONCE_LEN..], pt); // actually encrypted
        assert_eq!(decrypt(&key, &ct).unwrap(), pt);
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let key = generate_key();
        let a = encrypt(&key, b"same").unwrap();
        let b = encrypt(&key, b"same").unwrap();
        assert_ne!(a, b); // different nonce => different ciphertext
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let ct = encrypt(&generate_key(), b"data").unwrap();
        let err = decrypt(&generate_key(), &ct).unwrap_err();
        assert_eq!(err, CryptoError::Decrypt);
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let key = generate_key();
        let mut ct = encrypt(&key, b"data").unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0xFF; // flip a bit in the auth tag
        assert_eq!(decrypt(&key, &ct).unwrap_err(), CryptoError::Decrypt);
    }

    #[test]
    fn too_short_ciphertext_is_length_error() {
        assert_eq!(
            decrypt(&generate_key(), b"abc").unwrap_err(),
            CryptoError::Length
        );
    }

    #[test]
    fn dek_wrap_unwrap_round_trip() {
        let kek = derive_key("pw", &generate_salt()).unwrap();
        let dek = generate_key();
        let wrapped = wrap_key(&kek, &dek).unwrap();
        assert_eq!(unwrap_key(&kek, &wrapped).unwrap(), dek);
    }

    #[test]
    fn unwrap_with_wrong_kek_fails() {
        let dek = generate_key();
        let wrapped = wrap_key(&generate_key(), &dek).unwrap();
        assert!(unwrap_key(&generate_key(), &wrapped).is_err());
    }

    #[test]
    fn recovery_key_encodes_and_decodes() {
        let rk = generate_key();
        let encoded = encode_recovery_key(&rk);
        assert_eq!(decode_recovery_key(&encoded).unwrap(), rk);
    }

    #[test]
    fn either_passphrase_or_recovery_unlocks_dek() {
        // Setup: one DEK wrapped two ways.
        let salt = generate_salt();
        let kek = derive_key("my passphrase", &salt).unwrap();
        let recovery = generate_key();
        let dek = generate_key();
        let wrapped_pw = wrap_key(&kek, &dek).unwrap();
        let wrapped_rec = wrap_key(&recovery, &dek).unwrap();

        // Unlock via passphrase.
        let kek2 = derive_key("my passphrase", &salt).unwrap();
        assert_eq!(unwrap_key(&kek2, &wrapped_pw).unwrap(), dek);
        // Unlock via recovery key.
        assert_eq!(unwrap_key(&recovery, &wrapped_rec).unwrap(), dek);
    }

    #[test]
    fn vault_seal_open_round_trip() {
        let vault = Vault::new(generate_key());
        let sealed = vault.seal(b"docs/a.txt", b"file contents").unwrap();
        assert_ne!(sealed, b"file contents");
        assert_eq!(
            vault.open(b"docs/a.txt", &sealed).unwrap(),
            b"file contents"
        );
    }

    #[test]
    fn vault_rejects_wrong_context() {
        let vault = Vault::new(generate_key());
        let sealed = vault.seal(b"docs/a.txt", b"secret").unwrap();
        // Same key, but a different path as AAD -> authentication fails.
        assert_eq!(
            vault.open(b"docs/evil.txt", &sealed).unwrap_err(),
            CryptoError::Decrypt
        );
    }
}
