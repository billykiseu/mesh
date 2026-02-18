use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::crypto::SessionKeys;
use crate::message::MeshMessage;

/// State of a connected peer.
#[derive(Debug)]
pub struct PeerState {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub addr: SocketAddr,
    pub last_seen: Instant,
    pub session_keys: Option<SessionKeys>,
    /// Channel to send messages to this peer's TCP write task.
    pub sender: mpsc::Sender<MeshMessage>,
    // Profile fields
    pub bio: String,
    pub capabilities: Vec<String>,
    pub is_gateway: bool,
}

impl PeerState {
    pub fn new(
        node_id: [u8; 32],
        display_name: String,
        addr: SocketAddr,
        sender: mpsc::Sender<MeshMessage>,
    ) -> Self {
        Self {
            node_id,
            display_name,
            addr,
            last_seen: Instant::now(),
            session_keys: None,
            sender,
            bio: String::new(),
            capabilities: Vec::new(),
            is_gateway: false,
        }
    }

    pub fn touch(&mut self) {
        self.last_seen = Instant::now();
    }

    pub fn is_alive(&self, timeout: std::time::Duration) -> bool {
        self.last_seen.elapsed() < timeout
    }

    pub fn node_id_short(&self) -> String {
        hex::encode(&self.node_id[..4])
    }
}

/// Manages the set of known connected peers.
pub struct PeerManager {
    peers: HashMap<[u8; 32], PeerState>,
}

impl PeerManager {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    pub fn add(&mut self, peer: PeerState) {
        tracing::info!("Peer added: {} ({})", peer.display_name, peer.node_id_short());
        self.peers.insert(peer.node_id, peer);
    }

    pub fn remove(&mut self, node_id: &[u8; 32]) -> Option<PeerState> {
        let peer = self.peers.remove(node_id);
        if let Some(ref p) = peer {
            tracing::info!("Peer removed: {} ({})", p.display_name, p.node_id_short());
        }
        peer
    }

    pub fn get(&self, node_id: &[u8; 32]) -> Option<&PeerState> {
        self.peers.get(node_id)
    }

    pub fn get_mut(&mut self, node_id: &[u8; 32]) -> Option<&mut PeerState> {
        self.peers.get_mut(node_id)
    }

    pub fn contains(&self, node_id: &[u8; 32]) -> bool {
        self.peers.contains_key(node_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &PeerState> {
        self.peers.values()
    }

    pub fn count(&self) -> usize {
        self.peers.len()
    }

    /// Remove peers that haven't been seen within the timeout.
    pub fn prune_stale(&mut self, timeout: std::time::Duration) -> Vec<[u8; 32]> {
        let stale: Vec<[u8; 32]> = self.peers.iter()
            .filter(|(_, p)| !p.is_alive(timeout))
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            self.remove(id);
        }
        stale
    }

    /// Get senders for all peers (for broadcasting).
    pub fn broadcast_senders(&self) -> Vec<([u8; 32], mpsc::Sender<MeshMessage>)> {
        self.peers.iter()
            .map(|(id, p)| (*id, p.sender.clone()))
            .collect()
    }

    /// Get a list of all peer IDs.
    pub fn peer_ids(&self) -> Vec<[u8; 32]> {
        self.peers.keys().copied().collect()
    }
}
