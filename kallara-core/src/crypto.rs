use crate::format::FormatError;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes128Gcm;
use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;

pub const KEY_SIZE: usize = 16; // 128-bit key
pub const SALT_SIZE: usize = 16;
pub const BASE_NONCE_SIZE: usize = 7;
pub const GCM_NONCE_SIZE: usize = 12;

/// Derives a 128-bit key using Argon2id (m = 64 MiB, t = 2, p = 4).
pub fn derive_key(passphrase: &str, salt: &[u8; SALT_SIZE]) -> Result<[u8; KEY_SIZE], FormatError> {
    let params = Params::new(64 * 1024, 2, 4, Some(KEY_SIZE))
        .map_err(|e| FormatError::Crypto(format!("Failed to configure Argon2 params: {}", e)))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; KEY_SIZE];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| FormatError::Crypto(format!("Key derivation failed: {}", e)))?;

    Ok(key)
}

/// Generates a cryptographically secure random 16-byte salt and 7-byte base nonce.
pub fn generate_crypto_params() -> ([u8; SALT_SIZE], [u8; BASE_NONCE_SIZE]) {
    let mut rng = rand::thread_rng();
    let mut salt = [0u8; SALT_SIZE];
    let mut nonce = [0u8; BASE_NONCE_SIZE];
    rng.fill(&mut salt);
    rng.fill(&mut nonce);
    (salt, nonce)
}

/// Computes frame nonce for STREAM construction:
/// 7 bytes base_nonce + 4 bytes big-endian counter + 1 byte 0x00.
pub fn compute_stream_nonce(base_nonce: &[u8; BASE_NONCE_SIZE], counter: u32) -> [u8; GCM_NONCE_SIZE] {
    let mut nonce = [0u8; GCM_NONCE_SIZE];
    nonce[0..7].copy_from_slice(base_nonce);
    nonce[7..11].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = 0;
    nonce
}

/// Computes the footer nonce: Base_Nonce ^ 0xFFFFFFFF
pub fn derive_footer_nonce(base_nonce: &[u8; BASE_NONCE_SIZE]) -> [u8; GCM_NONCE_SIZE] {
    let mut nonce = [0u8; GCM_NONCE_SIZE];
    nonce[0..7].copy_from_slice(base_nonce);
    nonce[0] ^= 0xFF;
    nonce[1] ^= 0xFF;
    nonce[2] ^= 0xFF;
    nonce[3] ^= 0xFF;
    nonce[7..12].copy_from_slice(&[0xFF; 5]);
    nonce
}

/// Encrypts a stream frame with AES-128-GCM.
pub fn encrypt_frame(
    key: &[u8; KEY_SIZE],
    base_nonce: &[u8; BASE_NONCE_SIZE],
    counter: u32,
    plaintext: &[u8],
) -> Result<Vec<u8>, FormatError> {
    let cipher = Aes128Gcm::new_from_slice(key)
        .map_err(|e| FormatError::Crypto(format!("Invalid key length: {}", e)))?;
    let nonce = compute_stream_nonce(base_nonce, counter);
    cipher
        .encrypt(&nonce.into(), plaintext)
        .map_err(|e| FormatError::Crypto(format!("Frame encryption error: {}", e)))
}

/// Decrypts a stream frame with AES-128-GCM.
pub fn decrypt_frame(
    key: &[u8; KEY_SIZE],
    base_nonce: &[u8; BASE_NONCE_SIZE],
    counter: u32,
    ciphertext: &[u8],
) -> Result<Vec<u8>, FormatError> {
    let cipher = Aes128Gcm::new_from_slice(key)
        .map_err(|e| FormatError::Crypto(format!("Invalid key length: {}", e)))?;
    let nonce = compute_stream_nonce(base_nonce, counter);
    cipher
        .decrypt(&nonce.into(), ciphertext)
        .map_err(|e| FormatError::Crypto(format!("Frame decryption/auth failed: {}", e)))
}

/// Encrypts the directory index footer.
pub fn encrypt_footer(
    key: &[u8; KEY_SIZE],
    base_nonce: &[u8; BASE_NONCE_SIZE],
    plaintext_footer: &[u8],
) -> Result<Vec<u8>, FormatError> {
    let cipher = Aes128Gcm::new_from_slice(key)
        .map_err(|e| FormatError::Crypto(format!("Invalid key length: {}", e)))?;
    let nonce = derive_footer_nonce(base_nonce);
    cipher
        .encrypt(&nonce.into(), plaintext_footer)
        .map_err(|e| FormatError::Crypto(format!("Footer encryption failed: {}", e)))
}

/// Decrypts the directory index footer.
pub fn decrypt_footer(
    key: &[u8; KEY_SIZE],
    base_nonce: &[u8; BASE_NONCE_SIZE],
    ciphertext_footer: &[u8],
) -> Result<Vec<u8>, FormatError> {
    let cipher = Aes128Gcm::new_from_slice(key)
        .map_err(|e| FormatError::Crypto(format!("Invalid key length: {}", e)))?;
    let nonce = derive_footer_nonce(base_nonce);
    cipher
        .decrypt(&nonce.into(), ciphertext_footer)
        .map_err(|e| FormatError::Crypto(format!("Footer decryption/auth failed (wrong passphrase?): {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kdf_and_encryption() {
        let (salt, base_nonce) = generate_crypto_params();
        let key = derive_key("correct-passphrase", &salt).unwrap();

        let data = b"Directory index footer test data";
        let encrypted = encrypt_footer(&key, &base_nonce, data).unwrap();
        let decrypted = decrypt_footer(&key, &base_nonce, &encrypted).unwrap();
        assert_eq!(decrypted, data);

        // Wrong passphrase must fail authentication
        let wrong_key = derive_key("wrong-passphrase", &salt).unwrap();
        assert!(decrypt_footer(&wrong_key, &base_nonce, &encrypted).is_err());
    }

    #[test]
    fn test_frame_crypto() {
        let (salt, base_nonce) = generate_crypto_params();
        let key = derive_key("stream-key", &salt).unwrap();

        let frame = vec![0xAB; 64 * 1024];
        let encrypted = encrypt_frame(&key, &base_nonce, 0, &frame).unwrap();
        assert_eq!(encrypted.len(), frame.len() + 16);

        let decrypted = decrypt_frame(&key, &base_nonce, 0, &encrypted).unwrap();
        assert_eq!(decrypted, frame);
    }
}
