use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Verifier, Signature};
use rand::rngs::OsRng;
use std::path::Path;
use anyhow::{Result, Context};

/// A node's identity, backed by an Ed25519 keypair.
/// The public key (32 bytes) serves as the unique node ID.
#[derive(Clone)]
pub struct NodeIdentity {
    signing_key: SigningKey,
    pub node_id: [u8; 32],
    pub display_name: String,
}

impl NodeIdentity {
    /// Generate a new random identity.
    pub fn generate(display_name: String) -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let node_id = signing_key.verifying_key().to_bytes();
        Self { signing_key, node_id, display_name }
    }

    /// Load identity from a file, or generate and save a new one.
    pub fn load_or_create(path: &Path, display_name: String) -> Result<Self> {
        if path.exists() {
            Self::load(path, display_name)
        } else {
            let identity = Self::generate(display_name);
            identity.save(path)?;
            Ok(identity)
        }
    }

    /// Save the secret key bytes to a file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.signing_key.to_bytes())
            .context("Failed to save identity key")
    }

    /// Load identity from a 32-byte secret key file.
    pub fn load(path: &Path, display_name: String) -> Result<Self> {
        let bytes = std::fs::read(path).context("Failed to read identity key")?;
        let key_bytes: [u8; 32] = bytes.try_into()
            .map_err(|_| anyhow::anyhow!("Invalid key file: expected 32 bytes"))?;
        let signing_key = SigningKey::from_bytes(&key_bytes);
        let node_id = signing_key.verifying_key().to_bytes();
        Ok(Self { signing_key, node_id, display_name })
    }

    /// Get the node ID as a hex string.
    pub fn node_id_hex(&self) -> String {
        hex::encode(self.node_id)
    }

    /// Get a short display of the node ID (first 8 hex chars).
    pub fn node_id_short(&self) -> String {
        hex::encode(&self.node_id[..4])
    }

    /// Sign a message.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    /// Verify a signature against a public key.
    pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> Result<()> {
        let verifying_key = VerifyingKey::from_bytes(public_key)
            .context("Invalid public key")?;
        let sig = Signature::from_bytes(signature);
        verifying_key.verify(message, &sig)
            .context("Signature verification failed")
    }

    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.node_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_generate_identity() {
        let id = NodeIdentity::generate("TestNode".into());
        assert_eq!(id.node_id.len(), 32);
        assert!(!id.node_id_hex().is_empty());
        assert_eq!(id.node_id_short().len(), 8);
    }

    #[test]
    fn test_sign_and_verify() {
        let id = NodeIdentity::generate("TestNode".into());
        let message = b"hello mesh network";
        let sig = id.sign(message);
        assert!(NodeIdentity::verify(&id.node_id, message, &sig).is_ok());
    }

    #[test]
    fn test_verify_wrong_message() {
        let id = NodeIdentity::generate("TestNode".into());
        let sig = id.sign(b"correct message");
        assert!(NodeIdentity::verify(&id.node_id, b"wrong message", &sig).is_err());
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("mesh_test_identity");
        let path = dir.join("test_key");
        let _ = std::fs::remove_file(&path);

        let id1 = NodeIdentity::generate("TestNode".into());
        id1.save(&path).unwrap();

        let id2 = NodeIdentity::load(&path, "TestNode".into()).unwrap();
        assert_eq!(id1.node_id, id2.node_id);

        // Sign with original, verify with loaded
        let sig = id1.sign(b"persistence test");
        assert!(NodeIdentity::verify(&id2.node_id, b"persistence test", &sig).is_ok());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_or_create() {
        let dir = std::env::temp_dir().join("mesh_test_identity_loc");
        let path = dir.join("test_key_loc");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);

        // First call creates
        let id1 = NodeIdentity::load_or_create(&path, "Node1".into()).unwrap();
        // Second call loads the same key
        let id2 = NodeIdentity::load_or_create(&path, "Node1".into()).unwrap();
        assert_eq!(id1.node_id, id2.node_id);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
