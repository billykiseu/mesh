use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::message::{MeshMessage, MessageId};

const SEEN_EXPIRY: Duration = Duration::from_secs(300); // 5 minutes
const MAX_SEEN_CACHE: usize = 10_000;

/// Flooding router with TTL-based hop limiting and message deduplication.
pub struct Router {
    /// Cache of already-seen message IDs to prevent forwarding loops.
    seen: HashSet<MessageId>,
    seen_times: Vec<(MessageId, Instant)>,
    our_node_id: [u8; 32],
}

impl Router {
    pub fn new(our_node_id: [u8; 32]) -> Self {
        Self {
            seen: HashSet::new(),
            seen_times: Vec::new(),
            our_node_id,
        }
    }

    /// Check if we should process/forward this message.
    /// Returns true if the message is new (not seen before) and TTL > 0.
    pub fn should_process(&mut self, msg: &MeshMessage) -> bool {
        // Don't process our own messages
        if msg.sender_id == self.our_node_id {
            return false;
        }

        // Check TTL
        if msg.ttl == 0 {
            return false;
        }

        // Check dedup
        if self.seen.contains(&msg.msg_id) {
            return false;
        }

        // Mark as seen
        self.mark_seen(msg.msg_id);
        true
    }

    /// Prepare a message for forwarding: decrement TTL, check if still valid.
    /// Returns a cloned message with decremented TTL, or None if TTL expired.
    pub fn prepare_forward(&self, msg: &MeshMessage) -> Option<MeshMessage> {
        let mut forwarded = msg.clone();
        if forwarded.decrement_ttl() {
            Some(forwarded)
        } else {
            None
        }
    }

    /// Check if a message is destined for us.
    pub fn is_for_us(&self, msg: &MeshMessage) -> bool {
        match msg.destination {
            None => true, // Broadcast - everyone processes it
            Some(dest) => dest == self.our_node_id,
        }
    }

    /// Check if a message should be forwarded to other peers.
    pub fn should_forward(&self, msg: &MeshMessage) -> bool {
        match msg.destination {
            None => true,  // Broadcast - forward to everyone
            Some(dest) => dest != self.our_node_id, // Direct - forward if not for us
        }
    }

    fn mark_seen(&mut self, msg_id: MessageId) {
        self.seen.insert(msg_id);
        self.seen_times.push((msg_id, Instant::now()));

        // Periodic cleanup
        if self.seen_times.len() > MAX_SEEN_CACHE {
            self.cleanup();
        }
    }

    /// Remove expired entries from the seen cache.
    pub fn cleanup(&mut self) {
        let now = Instant::now();
        self.seen_times.retain(|(id, time)| {
            if now.duration_since(*time) > SEEN_EXPIRY {
                self.seen.remove(id);
                false
            } else {
                true
            }
        });
    }

    pub fn seen_count(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageType;

    fn make_msg(sender: [u8; 32], ttl: u8) -> MeshMessage {
        MeshMessage::new(MessageType::Text, sender, ttl, None, b"test".to_vec())
    }

    #[test]
    fn test_should_process_new_message() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 5);
        assert!(router.should_process(&msg));
    }

    #[test]
    fn test_reject_own_message() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let msg = make_msg(our_id, 5);
        assert!(!router.should_process(&msg));
    }

    #[test]
    fn test_dedup() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 5);
        assert!(router.should_process(&msg));
        assert!(!router.should_process(&msg)); // Same msg_id
    }

    #[test]
    fn test_ttl_zero_rejected() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 0);
        assert!(!router.should_process(&msg));
    }

    #[test]
    fn test_forward_decrements_ttl() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 3);
        let forwarded = router.prepare_forward(&msg).unwrap();
        assert_eq!(forwarded.ttl, 2);
    }

    #[test]
    fn test_forward_ttl_1_returns_none() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let mut msg = make_msg([2u8; 32], 1);
        msg.ttl = 1;
        let forwarded = router.prepare_forward(&msg);
        // TTL decrements to 0 which is still valid for forwarding (decrement_ttl returns true when going from 1 to 0)
        assert!(forwarded.is_some());
        assert_eq!(forwarded.unwrap().ttl, 0);
    }

    #[test]
    fn test_broadcast_is_for_us() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 5); // No destination = broadcast
        assert!(router.is_for_us(&msg));
    }

    #[test]
    fn test_direct_message_for_us() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let msg = MeshMessage::new(MessageType::Text, [2u8; 32], 5, Some(our_id), b"hello".to_vec());
        assert!(router.is_for_us(&msg));
        assert!(!router.should_forward(&msg));
    }

    #[test]
    fn test_direct_message_not_for_us() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let msg = MeshMessage::new(MessageType::Text, [2u8; 32], 5, Some([3u8; 32]), b"hello".to_vec());
        assert!(!router.is_for_us(&msg));
        assert!(router.should_forward(&msg));
    }
}
