use serde::{Serialize, Deserialize};
use rand::RngCore;
use rand::rngs::OsRng;

/// Message types in the mesh protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageType {
    Discovery = 0x01,
    Ping = 0x02,
    Pong = 0x03,
    Text = 0x10,
    FileChunk = 0x20,
    Voice = 0x30,
    PeerExchange = 0x40,
    KeyExchange = 0x50,
}

/// A unique message ID (32 bytes random).
pub type MessageId = [u8; 32];

/// Generate a random message ID.
pub fn new_message_id() -> MessageId {
    let mut id = [0u8; 32];
    OsRng.fill_bytes(&mut id);
    id
}

/// The wire-format mesh message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMessage {
    pub msg_type: MessageType,
    pub sender_id: [u8; 32],
    pub msg_id: MessageId,
    pub ttl: u8,
    /// None = broadcast, Some = direct to this node ID
    pub destination: Option<[u8; 32]>,
    pub payload: Vec<u8>,
}

impl MeshMessage {
    pub fn new(
        msg_type: MessageType,
        sender_id: [u8; 32],
        ttl: u8,
        destination: Option<[u8; 32]>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            msg_type,
            sender_id,
            msg_id: new_message_id(),
            ttl,
            destination,
            payload,
        }
    }

    /// Create a text message (broadcast).
    pub fn text(sender_id: [u8; 32], text: &str) -> Self {
        Self::new(MessageType::Text, sender_id, 10, None, text.as_bytes().to_vec())
    }

    /// Create a direct text message.
    pub fn text_to(sender_id: [u8; 32], dest: [u8; 32], text: &str) -> Self {
        Self::new(MessageType::Text, sender_id, 10, Some(dest), text.as_bytes().to_vec())
    }

    /// Serialize to bytes using bincode.
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("Message serialization should not fail")
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }

    /// Create a length-prefixed frame: [4-byte big-endian length][message bytes]
    pub fn to_frame(&self) -> Vec<u8> {
        let msg_bytes = self.to_bytes();
        let len = msg_bytes.len() as u32;
        let mut frame = Vec::with_capacity(4 + msg_bytes.len());
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&msg_bytes);
        frame
    }

    /// Decrement TTL, returning false if the message has expired.
    pub fn decrement_ttl(&mut self) -> bool {
        if self.ttl == 0 {
            return false;
        }
        self.ttl -= 1;
        true
    }
}

/// Discovery announcement payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPayload {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub listen_port: u16,
    pub capabilities: Vec<String>,
}

impl DiscoveryPayload {
    pub fn new(node_id: [u8; 32], display_name: String, listen_port: u16) -> Self {
        Self {
            node_id,
            display_name,
            listen_port,
            capabilities: vec!["text".into()],
        }
    }

    pub fn to_message(&self) -> MeshMessage {
        let payload = bincode::serialize(self).expect("Discovery payload serialization failed");
        MeshMessage::new(
            MessageType::Discovery,
            self.node_id,
            1, // Discovery messages don't need to hop far
            None,
            payload,
        )
    }

    pub fn from_message(msg: &MeshMessage) -> Result<Self, bincode::Error> {
        bincode::deserialize(&msg.payload)
    }
}

/// Key exchange payload sent when establishing a peer session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyExchangePayload {
    pub x25519_public: [u8; 32],
}

impl KeyExchangePayload {
    pub fn to_message(&self, sender_id: [u8; 32], dest: [u8; 32]) -> MeshMessage {
        let payload = bincode::serialize(self).expect("KeyExchange serialization failed");
        MeshMessage::new(MessageType::KeyExchange, sender_id, 1, Some(dest), payload)
    }

    pub fn from_message(msg: &MeshMessage) -> Result<Self, bincode::Error> {
        bincode::deserialize(&msg.payload)
    }
}

/// Peer exchange payload: share known peers with neighbors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerExchangePayload {
    pub peers: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub addr: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_roundtrip() {
        let msg = MeshMessage::text([1u8; 32], "Hello world!");
        let bytes = msg.to_bytes();
        let decoded = MeshMessage::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.msg_type, MessageType::Text);
        assert_eq!(decoded.sender_id, [1u8; 32]);
        assert_eq!(decoded.ttl, 10);
        assert_eq!(decoded.payload, b"Hello world!");
        assert!(decoded.destination.is_none());
    }

    #[test]
    fn test_direct_message() {
        let msg = MeshMessage::text_to([1u8; 32], [2u8; 32], "DM");
        let bytes = msg.to_bytes();
        let decoded = MeshMessage::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.destination, Some([2u8; 32]));
    }

    #[test]
    fn test_frame_format() {
        let msg = MeshMessage::text([1u8; 32], "test");
        let frame = msg.to_frame();

        let len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(len + 4, frame.len());

        let decoded = MeshMessage::from_bytes(&frame[4..]).unwrap();
        assert_eq!(decoded.payload, b"test");
    }

    #[test]
    fn test_ttl_decrement() {
        let mut msg = MeshMessage::text([1u8; 32], "hop");
        assert_eq!(msg.ttl, 10);

        for i in (0..10).rev() {
            assert!(msg.decrement_ttl());
            assert_eq!(msg.ttl, i);
        }

        // TTL is 0, should return false
        assert!(!msg.decrement_ttl());
    }

    #[test]
    fn test_discovery_payload() {
        let payload = DiscoveryPayload::new([3u8; 32], "Node3".into(), 7332);
        let msg = payload.to_message();

        assert_eq!(msg.msg_type, MessageType::Discovery);
        let decoded = DiscoveryPayload::from_message(&msg).unwrap();
        assert_eq!(decoded.node_id, [3u8; 32]);
        assert_eq!(decoded.display_name, "Node3");
        assert_eq!(decoded.listen_port, 7332);
    }

    #[test]
    fn test_message_id_uniqueness() {
        let m1 = MeshMessage::text([1u8; 32], "a");
        let m2 = MeshMessage::text([1u8; 32], "a");
        assert_ne!(m1.msg_id, m2.msg_id);
    }

    #[test]
    fn test_compact_serialization() {
        let msg = MeshMessage::text([0u8; 32], "hi");
        let bytes = msg.to_bytes();
        // Should be compact - well under 200 bytes for a short message
        assert!(bytes.len() < 200, "Serialized size {} is not compact", bytes.len());
    }
}
