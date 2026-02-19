use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::message::{MeshMessage, MessageId, MessageType};

const SEEN_EXPIRY: Duration = Duration::from_secs(300); // 5 minutes
const MAX_SEEN_CACHE: usize = 10_000;

/// Mesh network statistics.
#[derive(Debug, Clone, Default)]
pub struct MeshStats {
    pub total_peers: u32,
    pub messages_relayed: u64,
    pub messages_received: u64,
    pub unique_nodes_seen: u32,
    pub total_hops_observed: u64,
    pub hop_count_samples: u64,
}

impl MeshStats {
    pub fn avg_hops(&self) -> f32 {
        if self.hop_count_samples == 0 {
            0.0
        } else {
            self.total_hops_observed as f32 / self.hop_count_samples as f32
        }
    }
}

/// Flooding router with TTL-based hop limiting and message deduplication.
pub struct Router {
    /// Cache of already-seen message IDs to prevent forwarding loops.
    seen: HashSet<MessageId>,
    seen_times: Vec<(MessageId, Instant)>,
    our_node_id: [u8; 32],
    /// Track all unique node IDs ever seen (including via relay).
    pub all_nodes_seen: HashSet<[u8; 32]>,
    pub stats: MeshStats,
}

impl Router {
    pub fn new(our_node_id: [u8; 32]) -> Self {
        let mut all_nodes_seen = HashSet::new();
        all_nodes_seen.insert(our_node_id);
        Self {
            seen: HashSet::new(),
            seen_times: Vec::new(),
            our_node_id,
            all_nodes_seen,
            stats: MeshStats::default(),
        }
    }

    /// Check if we should process/forward this message.
    /// Returns true if the message is new (not seen before) and TTL > 0.
    pub fn should_process(&mut self, msg: &MeshMessage) -> bool {
        // Don't process our own messages
        if msg.sender_id == self.our_node_id {
            return false;
        }

        // SOS messages get priority - process even if dedup cache is full
        let is_sos = msg.msg_type == MessageType::SOS;

        // Check TTL
        if msg.ttl == 0 {
            return false;
        }

        // Check dedup (SOS bypasses when cache is full)
        if self.seen.contains(&msg.msg_id) {
            return false;
        }

        // If cache is full and not SOS, reject
        if !is_sos && self.seen.len() >= MAX_SEEN_CACHE {
            self.cleanup();
            if self.seen.len() >= MAX_SEEN_CACHE {
                return false;
            }
        }

        // Mark as seen
        self.mark_seen(msg.msg_id);

        // Track this sender as a unique node
        self.all_nodes_seen.insert(msg.sender_id);
        self.stats.unique_nodes_seen = self.all_nodes_seen.len() as u32;

        // Track hop count: original TTL minus current TTL
        self.record_hops(msg);

        // Count received messages
        if self.is_for_us(msg) {
            self.stats.messages_received += 1;
        }

        true
    }

    /// Record hop information from a message's TTL.
    fn record_hops(&mut self, msg: &MeshMessage) {
        // Estimate original TTL based on message type
        let original_ttl: u8 = match msg.msg_type {
            MessageType::Text => 10,
            MessageType::PublicBroadcast => 50,
            MessageType::SOS => 255,
            MessageType::ProfileUpdate => 3,
            MessageType::Voice => 10,
            MessageType::VoiceStream | MessageType::CallStart | MessageType::CallEnd => 2,
            MessageType::FileOffer | MessageType::FileChunk | MessageType::FileAccept => 10,
            MessageType::ReadReceipt | MessageType::GroupMessage | MessageType::Disappearing => 10,
            MessageType::TypingStart | MessageType::TypingStop => 1,
            MessageType::CheckIn | MessageType::Triage | MessageType::ResourceReq => 50,
            MessageType::GroupJoin | MessageType::GroupLeave => 10,
            _ => return, // Don't track hops for Discovery/Ping/Pong
        };

        let hops = original_ttl.saturating_sub(msg.ttl) as u64;
        if hops > 0 {
            self.stats.total_hops_observed += hops;
            self.stats.hop_count_samples += 1;
        }
    }

    /// Prepare a message for forwarding: decrement TTL, check if still valid.
    /// Returns a cloned message with decremented TTL, or None if TTL expired.
    pub fn prepare_forward(&mut self, msg: &MeshMessage) -> Option<MeshMessage> {
        let mut forwarded = msg.clone();
        if forwarded.decrement_ttl() {
            self.stats.messages_relayed += 1;
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

// ---------------------------------------------------------------------------
// Routing table for smarter multi-hop directed messages
// ---------------------------------------------------------------------------

const ROUTE_EXPIRY: Duration = Duration::from_secs(120);

struct RouteEntry {
    next_hop: [u8; 32],
    hop_count: u8,
    last_updated: Instant,
}

/// Routing table: tracks best next-hop for each known destination.
pub struct RoutingTable {
    routes: HashMap<[u8; 32], RouteEntry>,
}

impl RoutingTable {
    pub fn new() -> Self {
        Self { routes: HashMap::new() }
    }

    /// Update routing table with information from a received message.
    /// The sender is `via` (direct peer), the message originated from `origin` with `hop_count` hops.
    pub fn update_route(&mut self, origin: [u8; 32], via: [u8; 32], hop_count: u8) {
        let entry = self.routes.entry(origin).or_insert(RouteEntry {
            next_hop: via,
            hop_count,
            last_updated: Instant::now(),
        });
        // Update if shorter path or same path refreshed
        if hop_count <= entry.hop_count || entry.last_updated.elapsed() > ROUTE_EXPIRY {
            entry.next_hop = via;
            entry.hop_count = hop_count;
            entry.last_updated = Instant::now();
        }
    }

    /// Look up the best next hop for a destination.
    pub fn lookup(&self, dest: &[u8; 32]) -> Option<[u8; 32]> {
        self.routes.get(dest).and_then(|e| {
            if e.last_updated.elapsed() < ROUTE_EXPIRY {
                Some(e.next_hop)
            } else {
                None
            }
        })
    }

    /// Remove expired routes.
    pub fn cleanup(&mut self) {
        self.routes.retain(|_, e| e.last_updated.elapsed() < ROUTE_EXPIRY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 3);
        let forwarded = router.prepare_forward(&msg).unwrap();
        assert_eq!(forwarded.ttl, 2);
    }

    #[test]
    fn test_forward_ttl_1_returns_none() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let mut msg = make_msg([2u8; 32], 1);
        msg.ttl = 1;
        let forwarded = router.prepare_forward(&msg);
        assert!(forwarded.is_some());
        assert_eq!(forwarded.unwrap().ttl, 0);
    }

    #[test]
    fn test_broadcast_is_for_us() {
        let our_id = [1u8; 32];
        let router = Router::new(our_id);

        let msg = make_msg([2u8; 32], 5);
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

    #[test]
    fn test_stats_tracking() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let msg1 = make_msg([2u8; 32], 5);
        router.should_process(&msg1);

        let msg2 = make_msg([3u8; 32], 5);
        router.should_process(&msg2);

        // Our node + 2 senders
        assert_eq!(router.stats.unique_nodes_seen, 3);
        assert_eq!(router.stats.messages_received, 2); // Both are broadcasts
    }

    #[test]
    fn test_sos_priority() {
        let our_id = [1u8; 32];
        let mut router = Router::new(our_id);

        let sos = MeshMessage::new(MessageType::SOS, [2u8; 32], 255, None, b"help".to_vec());
        assert!(router.should_process(&sos));
    }
}
