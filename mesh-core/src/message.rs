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
    PublicBroadcast = 0x11,
    SOS = 0x12,
    FileChunk = 0x20,
    FileOffer = 0x21,
    FileAccept = 0x22,
    Voice = 0x30,
    VoiceStream = 0x31,
    CallStart = 0x32,
    CallEnd = 0x33,
    PeerExchange = 0x40,
    KeyExchange = 0x50,
    ProfileUpdate = 0x60,
    ReadReceipt = 0x13,
    TypingStart = 0x14,
    TypingStop = 0x15,
    GroupMessage = 0x16,
    GroupJoin = 0x17,
    GroupLeave = 0x18,
    CheckIn = 0x19,
    Triage = 0x1A,
    ResourceReq = 0x1B,
    Disappearing = 0x1C,
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
    /// Ed25519 signature over (msg_type, sender_id, msg_id, payload)
    #[serde(default)]
    pub signature: Option<Vec<u8>>,
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
            signature: None,
        }
    }

    /// Compute the signing payload: (msg_type as u8) + sender_id + msg_id + payload hash.
    pub fn signing_bytes(&self) -> Vec<u8> {
        use sha2::{Sha256, Digest};
        let mut buf = Vec::new();
        buf.push(self.msg_type as u8);
        buf.extend_from_slice(&self.sender_id);
        buf.extend_from_slice(&self.msg_id);
        let payload_hash = Sha256::digest(&self.payload);
        buf.extend_from_slice(&payload_hash);
        buf
    }

    /// Create a text message (broadcast).
    pub fn text(sender_id: [u8; 32], text: &str) -> Self {
        Self::new(MessageType::Text, sender_id, 10, None, text.as_bytes().to_vec())
    }

    /// Create a direct text message.
    pub fn text_to(sender_id: [u8; 32], dest: [u8; 32], text: &str) -> Self {
        Self::new(MessageType::Text, sender_id, 10, Some(dest), text.as_bytes().to_vec())
    }

    /// Create a public broadcast message (higher TTL for wider reach).
    pub fn public_broadcast(sender_id: [u8; 32], text: &str) -> Self {
        Self::new(MessageType::PublicBroadcast, sender_id, 50, None, text.as_bytes().to_vec())
    }

    /// Create an SOS emergency broadcast (max TTL).
    pub fn sos(sender_id: [u8; 32], payload: &SOSPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("SOS serialization failed");
        Self::new(MessageType::SOS, sender_id, 255, None, bytes)
    }

    /// Create a profile update broadcast.
    pub fn profile_update(sender_id: [u8; 32], payload: &ProfilePayload) -> Self {
        let bytes = bincode::serialize(payload).expect("Profile serialization failed");
        Self::new(MessageType::ProfileUpdate, sender_id, 3, None, bytes)
    }

    /// Create a file offer (direct to recipient).
    pub fn file_offer(sender_id: [u8; 32], dest: [u8; 32], payload: &FileOfferPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("FileOffer serialization failed");
        Self::new(MessageType::FileOffer, sender_id, 10, Some(dest), bytes)
    }

    /// Create a file chunk (direct to recipient).
    pub fn file_chunk(sender_id: [u8; 32], dest: [u8; 32], payload: &FileChunkPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("FileChunk serialization failed");
        Self::new(MessageType::FileChunk, sender_id, 10, Some(dest), bytes)
    }

    /// Create a file accept response (direct to sender).
    pub fn file_accept(sender_id: [u8; 32], dest: [u8; 32], file_id: [u8; 16]) -> Self {
        let payload = FileAcceptPayload { file_id };
        let bytes = bincode::serialize(&payload).expect("FileAccept serialization failed");
        Self::new(MessageType::FileAccept, sender_id, 10, Some(dest), bytes)
    }

    /// Create a voice note message.
    pub fn voice_note(sender_id: [u8; 32], dest: Option<[u8; 32]>, payload: &VoiceNotePayload) -> Self {
        let bytes = bincode::serialize(payload).expect("Voice serialization failed");
        Self::new(MessageType::Voice, sender_id, 10, dest, bytes)
    }

    /// Create a voice stream frame (direct, low TTL for LAN).
    pub fn voice_stream(sender_id: [u8; 32], dest: [u8; 32], payload: &VoiceStreamPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("VoiceStream serialization failed");
        Self::new(MessageType::VoiceStream, sender_id, 2, Some(dest), bytes)
    }

    /// Create a call start signal.
    pub fn call_start(sender_id: [u8; 32], dest: [u8; 32], payload: &CallControlPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("CallStart serialization failed");
        Self::new(MessageType::CallStart, sender_id, 2, Some(dest), bytes)
    }

    /// Create a call end signal.
    pub fn call_end(sender_id: [u8; 32], dest: [u8; 32], payload: &CallControlPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("CallEnd serialization failed");
        Self::new(MessageType::CallEnd, sender_id, 2, Some(dest), bytes)
    }

    /// Create a read receipt message.
    pub fn read_receipt(sender_id: [u8; 32], dest: [u8; 32], original_msg_id: MessageId) -> Self {
        let payload = ReadReceiptPayload { original_msg_id };
        let bytes = bincode::serialize(&payload).expect("ReadReceipt serialization failed");
        Self::new(MessageType::ReadReceipt, sender_id, 10, Some(dest), bytes)
    }

    /// Create a typing start indicator.
    pub fn typing_start(sender_id: [u8; 32], dest: Option<[u8; 32]>) -> Self {
        Self::new(MessageType::TypingStart, sender_id, 1, dest, vec![])
    }

    /// Create a typing stop indicator.
    pub fn typing_stop(sender_id: [u8; 32], dest: Option<[u8; 32]>) -> Self {
        Self::new(MessageType::TypingStop, sender_id, 1, dest, vec![])
    }

    /// Create a group message.
    pub fn group_message(sender_id: [u8; 32], group_name: &str, content: &str) -> Self {
        let payload = GroupPayload { group_name: group_name.to_string(), content: content.to_string() };
        let bytes = bincode::serialize(&payload).expect("GroupPayload serialization failed");
        Self::new(MessageType::GroupMessage, sender_id, 10, None, bytes)
    }

    /// Create a group join announcement.
    pub fn group_join(sender_id: [u8; 32], group_name: &str) -> Self {
        let payload = GroupControlPayload { group_name: group_name.to_string() };
        let bytes = bincode::serialize(&payload).expect("GroupControl serialization failed");
        Self::new(MessageType::GroupJoin, sender_id, 10, None, bytes)
    }

    /// Create a group leave announcement.
    pub fn group_leave(sender_id: [u8; 32], group_name: &str) -> Self {
        let payload = GroupControlPayload { group_name: group_name.to_string() };
        let bytes = bincode::serialize(&payload).expect("GroupControl serialization failed");
        Self::new(MessageType::GroupLeave, sender_id, 10, None, bytes)
    }

    /// Create a triage tag message (wide broadcast).
    pub fn triage(sender_id: [u8; 32], payload: &TriagePayload) -> Self {
        let bytes = bincode::serialize(payload).expect("Triage serialization failed");
        Self::new(MessageType::Triage, sender_id, 50, None, bytes)
    }

    /// Create a resource request message (wide broadcast).
    pub fn resource_request(sender_id: [u8; 32], payload: &ResourceRequestPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("ResourceRequest serialization failed");
        Self::new(MessageType::ResourceReq, sender_id, 50, None, bytes)
    }

    /// Create a check-in message (wide broadcast).
    pub fn check_in(sender_id: [u8; 32], payload: &CheckInPayload) -> Self {
        let bytes = bincode::serialize(payload).expect("CheckIn serialization failed");
        Self::new(MessageType::CheckIn, sender_id, 50, None, bytes)
    }

    /// Create a disappearing message.
    pub fn disappearing(sender_id: [u8; 32], dest: Option<[u8; 32]>, text: &str, ttl_seconds: u32) -> Self {
        let payload = DisappearingPayload { text: text.to_string(), ttl_seconds };
        let bytes = bincode::serialize(&payload).expect("Disappearing serialization failed");
        Self::new(MessageType::Disappearing, sender_id, 10, dest, bytes)
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

// ---------------------------------------------------------------------------
// Payload structs
// ---------------------------------------------------------------------------

/// Profile update payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePayload {
    pub display_name: String,
    pub bio: String,
    pub capabilities: Vec<String>,
}

/// File transfer offer payload (metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOfferPayload {
    pub file_id: [u8; 16],
    pub filename: String,
    pub size_bytes: u64,
    pub chunk_count: u32,
    pub sha256_hash: [u8; 32],
}

/// File chunk data payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunkPayload {
    pub file_id: [u8; 16],
    pub sequence: u32,
    pub data: Vec<u8>,
}

/// File accept response payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAcceptPayload {
    pub file_id: [u8; 16],
}

/// Voice note payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceNotePayload {
    pub duration_ms: u32,
    pub audio_data: Vec<u8>,
}

/// Voice stream frame payload (PTT).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStreamPayload {
    pub stream_id: [u8; 16],
    pub sequence: u32,
    pub audio_frame: Vec<u8>,
}

/// Call control payload (start/end).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallControlPayload {
    pub stream_id: [u8; 16],
}

/// SOS emergency broadcast payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SOSPayload {
    pub text: String,
    pub location: Option<(f64, f64)>,
}

// ---------------------------------------------------------------------------
// New payload structs (groups, emergency, disappearing, read receipts)
// ---------------------------------------------------------------------------

/// Read receipt payload — confirms delivery/read of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadReceiptPayload {
    pub original_msg_id: MessageId,
}

/// Group message payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupPayload {
    pub group_name: String,
    pub content: String,
}

/// Group control payload (join/leave).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupControlPayload {
    pub group_name: String,
}

/// START triage levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriageLevel {
    Black,
    Red,
    Yellow,
    Green,
}

impl TriageLevel {
    pub fn label(&self) -> &'static str {
        match self {
            TriageLevel::Black => "BLACK",
            TriageLevel::Red => "RED",
            TriageLevel::Yellow => "YELLOW",
            TriageLevel::Green => "GREEN",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "black" => Some(TriageLevel::Black),
            "red" => Some(TriageLevel::Red),
            "yellow" => Some(TriageLevel::Yellow),
            "green" => Some(TriageLevel::Green),
            _ => None,
        }
    }
}

/// Triage tag payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriagePayload {
    pub level: TriageLevel,
    pub victim_id: String,
    pub notes: String,
    pub location: Option<(f64, f64)>,
}

/// Structured resource request payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequestPayload {
    pub category: String,
    pub description: String,
    pub urgency: u8,
    pub location: Option<(f64, f64)>,
    pub quantity: u32,
}

/// Safety check-in payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckInPayload {
    pub status: String,
    pub location: Option<(f64, f64)>,
    pub message: String,
}

/// Disappearing message payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisappearingPayload {
    pub text: String,
    pub ttl_seconds: u32,
}

// ---------------------------------------------------------------------------
// Discovery & exchange payloads
// ---------------------------------------------------------------------------

/// Discovery announcement payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPayload {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub listen_port: u16,
    pub capabilities: Vec<String>,
    pub has_internet: bool,
}

impl DiscoveryPayload {
    pub fn new(node_id: [u8; 32], display_name: String, listen_port: u16, has_internet: bool) -> Self {
        Self {
            node_id,
            display_name,
            listen_port,
            capabilities: vec!["text".into(), "voice".into(), "file".into()],
            has_internet,
        }
    }

    pub fn to_message(&self) -> MeshMessage {
        let payload = bincode::serialize(self).expect("Discovery payload serialization failed");
        MeshMessage::new(
            MessageType::Discovery,
            self.node_id,
            1,
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

        assert!(!msg.decrement_ttl());
    }

    #[test]
    fn test_discovery_payload() {
        let payload = DiscoveryPayload::new([3u8; 32], "Node3".into(), 7332, false);
        let msg = payload.to_message();

        assert_eq!(msg.msg_type, MessageType::Discovery);
        let decoded = DiscoveryPayload::from_message(&msg).unwrap();
        assert_eq!(decoded.node_id, [3u8; 32]);
        assert_eq!(decoded.display_name, "Node3");
        assert_eq!(decoded.listen_port, 7332);
        assert!(!decoded.has_internet);
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
        assert!(bytes.len() < 200, "Serialized size {} is not compact", bytes.len());
    }

    #[test]
    fn test_public_broadcast() {
        let msg = MeshMessage::public_broadcast([1u8; 32], "emergency info");
        assert_eq!(msg.msg_type, MessageType::PublicBroadcast);
        assert_eq!(msg.ttl, 50);
        assert!(msg.destination.is_none());
    }

    #[test]
    fn test_sos_roundtrip() {
        let sos = SOSPayload {
            text: "Need help!".into(),
            location: Some((37.7749, -122.4194)),
        };
        let msg = MeshMessage::sos([1u8; 32], &sos);
        assert_eq!(msg.msg_type, MessageType::SOS);
        assert_eq!(msg.ttl, 255);

        let decoded: SOSPayload = bincode::deserialize(&msg.payload).unwrap();
        assert_eq!(decoded.text, "Need help!");
        assert_eq!(decoded.location, Some((37.7749, -122.4194)));
    }

    #[test]
    fn test_profile_roundtrip() {
        let profile = ProfilePayload {
            display_name: "Alice".into(),
            bio: "Hello world".into(),
            capabilities: vec!["text".into(), "voice".into()],
        };
        let msg = MeshMessage::profile_update([1u8; 32], &profile);
        assert_eq!(msg.msg_type, MessageType::ProfileUpdate);

        let decoded: ProfilePayload = bincode::deserialize(&msg.payload).unwrap();
        assert_eq!(decoded.display_name, "Alice");
        assert_eq!(decoded.bio, "Hello world");
    }

    #[test]
    fn test_file_offer_roundtrip() {
        let offer = FileOfferPayload {
            file_id: [42u8; 16],
            filename: "test.txt".into(),
            size_bytes: 1024,
            chunk_count: 1,
            sha256_hash: [0u8; 32],
        };
        let msg = MeshMessage::file_offer([1u8; 32], [2u8; 32], &offer);
        assert_eq!(msg.msg_type, MessageType::FileOffer);

        let decoded: FileOfferPayload = bincode::deserialize(&msg.payload).unwrap();
        assert_eq!(decoded.filename, "test.txt");
        assert_eq!(decoded.size_bytes, 1024);
    }

    #[test]
    fn test_voice_note_roundtrip() {
        let voice = VoiceNotePayload {
            duration_ms: 5000,
            audio_data: vec![1, 2, 3, 4, 5],
        };
        let msg = MeshMessage::voice_note([1u8; 32], Some([2u8; 32]), &voice);
        assert_eq!(msg.msg_type, MessageType::Voice);

        let decoded: VoiceNotePayload = bincode::deserialize(&msg.payload).unwrap();
        assert_eq!(decoded.duration_ms, 5000);
        assert_eq!(decoded.audio_data, vec![1, 2, 3, 4, 5]);
    }
}
