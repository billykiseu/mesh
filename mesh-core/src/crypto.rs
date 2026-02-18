use x25519_dalek::{PublicKey, StaticSecret};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rand::rngs::OsRng;
use rand::RngCore;
use anyhow::Result;

/// A session key derived from X25519 key exchange between two peers.
#[derive(Clone, Debug)]
pub struct SessionKeys {
    /// The shared secret used for encryption.
    shared_key: [u8; 32],
    /// Our public key for this session.
    pub our_public: [u8; 32],
}

/// Generate an X25519 static secret and its public key.
pub fn generate_x25519_keypair() -> (StaticSecret, PublicKey) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret, public)
}

impl SessionKeys {
    /// Perform X25519 key exchange: given our secret and the peer's public key,
    /// derive a shared session key.
    pub fn from_exchange(our_secret: &StaticSecret, their_public: &[u8; 32]) -> Self {
        let their_pk = PublicKey::from(*their_public);
        let shared = our_secret.diffie_hellman(&their_pk);
        // Use SHA-256 to derive a proper key from the shared secret
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(shared.as_bytes());
        let derived: [u8; 32] = hasher.finalize().into();

        let our_public = PublicKey::from(our_secret).to_bytes();
        Self {
            shared_key: derived,
            our_public,
        }
    }

    /// Encrypt a plaintext message using ChaCha20-Poly1305.
    /// Returns: [12-byte nonce][ciphertext+tag]
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(&self.shared_key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt a message. Input format: [12-byte nonce][ciphertext+tag]
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 12 {
            anyhow::bail!("Ciphertext too short");
        }

        let cipher = ChaCha20Poly1305::new_from_slice(&self.shared_key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

        let nonce = Nonce::from_slice(&data[..12]);
        let ciphertext = &data[12..];

        cipher.decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))
    }
}

/// Encrypt with a raw 32-byte key (for broadcast/mesh-wide key).
pub fn encrypt_with_key(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt with a raw 32-byte key.
pub fn decrypt_with_key(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 12 {
        anyhow::bail!("Ciphertext too short");
    }

    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| anyhow::anyhow!("Failed to create cipher: {}", e))?;

    let nonce = Nonce::from_slice(&data[..12]);
    let ciphertext = &data[12..];

    cipher.decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_x25519_key_exchange() {
        let (secret_a, public_a) = generate_x25519_keypair();
        let (secret_b, public_b) = generate_x25519_keypair();

        let session_a = SessionKeys::from_exchange(&secret_a, &public_b.to_bytes());
        let session_b = SessionKeys::from_exchange(&secret_b, &public_a.to_bytes());

        // Both sides should derive the same shared key
        assert_eq!(session_a.shared_key, session_b.shared_key);
    }

    #[test]
    fn test_encrypt_decrypt() {
        let (secret_a, public_a) = generate_x25519_keypair();
        let (secret_b, public_b) = generate_x25519_keypair();

        let session_a = SessionKeys::from_exchange(&secret_a, &public_b.to_bytes());
        let session_b = SessionKeys::from_exchange(&secret_b, &public_a.to_bytes());

        let plaintext = b"Hello, secure mesh!";
        let encrypted = session_a.encrypt(plaintext).unwrap();
        let decrypted = session_b.decrypt(&encrypted).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_encrypt_decrypt_empty() {
        let (secret_a, public_a) = generate_x25519_keypair();
        let (secret_b, public_b) = generate_x25519_keypair();

        let session_a = SessionKeys::from_exchange(&secret_a, &public_b.to_bytes());
        let session_b = SessionKeys::from_exchange(&secret_b, &public_a.to_bytes());

        let encrypted = session_a.encrypt(b"").unwrap();
        let decrypted = session_b.decrypt(&encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_wrong_key_fails() {
        let (secret_a, _) = generate_x25519_keypair();
        let (_, public_b) = generate_x25519_keypair();
        let (secret_c, _) = generate_x25519_keypair();

        let session_a = SessionKeys::from_exchange(&secret_a, &public_b.to_bytes());
        let session_c = SessionKeys::from_exchange(&secret_c, &public_b.to_bytes());

        let encrypted = session_a.encrypt(b"secret data").unwrap();
        assert!(session_c.decrypt(&encrypted).is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let (secret_a, public_a) = generate_x25519_keypair();
        let (secret_b, public_b) = generate_x25519_keypair();

        let session_a = SessionKeys::from_exchange(&secret_a, &public_b.to_bytes());
        let session_b = SessionKeys::from_exchange(&secret_b, &public_a.to_bytes());

        let mut encrypted = session_a.encrypt(b"integrity check").unwrap();
        // Flip a byte in the ciphertext (after the 12-byte nonce)
        if encrypted.len() > 13 {
            encrypted[13] ^= 0xFF;
        }
        assert!(session_b.decrypt(&encrypted).is_err());
    }

    #[test]
    fn test_broadcast_key_encrypt_decrypt() {
        let key = [42u8; 32];
        let plaintext = b"broadcast message to all nodes";

        let encrypted = encrypt_with_key(&key, plaintext).unwrap();
        let decrypted = decrypt_with_key(&key, &encrypted).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_broadcast_wrong_key_fails() {
        let key1 = [42u8; 32];
        let key2 = [99u8; 32];

        let encrypted = encrypt_with_key(&key1, b"secret").unwrap();
        assert!(decrypt_with_key(&key2, &encrypted).is_err());
    }
}
