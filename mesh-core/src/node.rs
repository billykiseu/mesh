use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn, debug};
use anyhow::Result;

use crate::crypto::{generate_x25519_keypair, SessionKeys};
use crate::discovery::DiscoveryService;
use crate::file_transfer::FileTransferManager;
use crate::gateway;
use crate::identity::NodeIdentity;
use crate::message::*;
use crate::peer::{PeerManager, PeerState};
use crate::router::Router;
use crate::transport::{TcpTransport, IncomingMessage, InboundConnection};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const GATEWAY_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const PEER_TIMEOUT: Duration = Duration::from_secs(30);
const TCP_PORT: u16 = 7332;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Mesh network statistics (public-facing).
#[derive(Debug, Clone, Default)]
pub struct MeshStats {
    pub total_peers: u32,
    pub messages_relayed: u64,
    pub messages_received: u64,
    pub unique_nodes_seen: u32,
    pub avg_hops: f32,
}

/// A peer entry for the peer list query.
#[derive(Debug, Clone)]
pub struct PeerListEntry {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub addr: String,
    pub is_gateway: bool,
    pub bio: String,
}

/// Events emitted by the node for the application layer.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    Started {
        node_id: String,
    },
    PeerConnected {
        node_id: [u8; 32],
        display_name: String,
    },
    PeerDisconnected {
        node_id: [u8; 32],
    },
    MessageReceived {
        sender_id: [u8; 32],
        sender_name: String,
        content: String,
    },
    // File transfer events
    FileOffered {
        sender_id: [u8; 32],
        sender_name: String,
        file_id: [u8; 16],
        filename: String,
        size: u64,
    },
    FileProgress {
        file_id: [u8; 16],
        pct: u8,
    },
    FileComplete {
        file_id: [u8; 16],
        path: String,
    },
    // Voice events
    VoiceReceived {
        sender_id: [u8; 32],
        sender_name: String,
        audio_data: Vec<u8>,
        duration_ms: u32,
    },
    // PTT call events
    IncomingCall {
        peer: [u8; 32],
        peer_name: String,
    },
    AudioFrame {
        peer: [u8; 32],
        data: Vec<u8>,
    },
    CallEnded {
        peer: [u8; 32],
    },
    // Profile events
    ProfileUpdated {
        node_id: [u8; 32],
        name: String,
        bio: String,
    },
    // Gateway events
    GatewayFound {
        node_id: [u8; 32],
        display_name: String,
    },
    GatewayLost {
        node_id: [u8; 32],
    },
    // Stats
    Stats {
        stats: MeshStats,
    },
    PeerList {
        peers: Vec<PeerListEntry>,
    },
    // Public broadcast / SOS
    PublicBroadcast {
        sender_id: [u8; 32],
        sender_name: String,
        text: String,
    },
    SOSReceived {
        sender_id: [u8; 32],
        sender_name: String,
        text: String,
        location: Option<(f64, f64)>,
    },
    // Lifecycle
    Nuked,
    Stopped,
}

/// Configuration for the mesh node.
pub struct NodeConfig {
    pub display_name: String,
    pub listen_port: u16,
    pub key_path: PathBuf,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            display_name: "MeshNode".into(),
            listen_port: TCP_PORT,
            key_path: PathBuf::from("mesh_identity.key"),
        }
    }
}

/// Commands sent from the application layer to the node.
pub enum NodeCommand {
    SendBroadcast { text: String },
    SendDirect { dest: [u8; 32], text: String },
    // File transfer
    SendFile { dest: [u8; 32], file_path: String },
    AcceptFile { file_id: [u8; 16] },
    // Voice
    SendVoice { dest: Option<[u8; 32]>, audio_data: Vec<u8>, duration_ms: u32 },
    // PTT
    StartVoiceCall { peer: [u8; 32] },
    EndVoiceCall,
    SendAudioFrame { peer: [u8; 32], data: Vec<u8> },
    // Profile
    UpdateProfile { name: String, bio: String },
    // Public broadcast / SOS
    SendPublicBroadcast { text: String },
    SendSOS { text: String, location: Option<(f64, f64)> },
    // Admin
    Nuke,
    Shutdown,
    GetStats,
    GetPeers,
}

/// A handle for sending commands from the application layer.
#[derive(Clone)]
pub struct NodeHandle {
    command_tx: mpsc::Sender<NodeCommand>,
}

impl NodeHandle {
    pub async fn send_broadcast(&self, text: &str) -> Result<()> {
        self.command_tx.send(NodeCommand::SendBroadcast {
            text: text.to_string(),
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_direct(&self, dest: [u8; 32], text: &str) -> Result<()> {
        self.command_tx.send(NodeCommand::SendDirect {
            dest,
            text: text.to_string(),
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_file(&self, dest: [u8; 32], file_path: &str) -> Result<()> {
        self.command_tx.send(NodeCommand::SendFile {
            dest,
            file_path: file_path.to_string(),
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn accept_file(&self, file_id: [u8; 16]) -> Result<()> {
        self.command_tx.send(NodeCommand::AcceptFile { file_id })
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_voice(&self, dest: Option<[u8; 32]>, audio_data: Vec<u8>, duration_ms: u32) -> Result<()> {
        self.command_tx.send(NodeCommand::SendVoice { dest, audio_data, duration_ms })
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn start_voice_call(&self, peer: [u8; 32]) -> Result<()> {
        self.command_tx.send(NodeCommand::StartVoiceCall { peer })
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn end_voice_call(&self) -> Result<()> {
        self.command_tx.send(NodeCommand::EndVoiceCall)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_audio_frame(&self, peer: [u8; 32], data: Vec<u8>) -> Result<()> {
        self.command_tx.send(NodeCommand::SendAudioFrame { peer, data })
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn update_profile(&self, name: &str, bio: &str) -> Result<()> {
        self.command_tx.send(NodeCommand::UpdateProfile {
            name: name.to_string(),
            bio: bio.to_string(),
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_public_broadcast(&self, text: &str) -> Result<()> {
        self.command_tx.send(NodeCommand::SendPublicBroadcast {
            text: text.to_string(),
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn send_sos(&self, text: &str, location: Option<(f64, f64)>) -> Result<()> {
        self.command_tx.send(NodeCommand::SendSOS {
            text: text.to_string(),
            location,
        }).await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn nuke(&self) -> Result<()> {
        self.command_tx.send(NodeCommand::Nuke)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.command_tx.send(NodeCommand::Shutdown)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn get_stats(&self) -> Result<()> {
        self.command_tx.send(NodeCommand::GetStats)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    pub async fn get_peers(&self) -> Result<()> {
        self.command_tx.send(NodeCommand::GetPeers)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }

    /// Raw command send for FFI/custom commands.
    pub async fn send_command(&self, cmd: NodeCommand) -> Result<()> {
        self.command_tx.send(cmd)
            .await.map_err(|_| anyhow::anyhow!("Node command channel closed"))
    }
}

// ---------------------------------------------------------------------------
// Node startup
// ---------------------------------------------------------------------------

/// Create and start a full mesh node, returning handles for the application.
pub async fn start_mesh_node(config: NodeConfig) -> Result<(NodeIdentity, NodeHandle, mpsc::Receiver<NodeEvent>)> {
    let identity = NodeIdentity::load_or_create(&config.key_path, config.display_name.clone())?;
    info!("Node identity: {} ({})", identity.node_id_short(), identity.display_name);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (event_tx, event_rx) = mpsc::channel::<NodeEvent>(256);
    let (command_tx, mut command_rx) = mpsc::channel::<NodeCommand>(256);
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<IncomingMessage>(256);
    let (inbound_conn_tx, mut inbound_conn_rx) = mpsc::channel::<InboundConnection>(64);

    // Check internet connectivity
    let has_internet = gateway::check_internet();
    info!("Internet gateway: {}", has_internet);

    // Start TCP listener
    let transport = TcpTransport::new(config.listen_port);
    transport.start_listener(incoming_tx.clone(), inbound_conn_tx, shutdown_rx.clone()).await?;

    // Start discovery
    let discovery = DiscoveryService::new(
        identity.node_id,
        identity.display_name.clone(),
        config.listen_port,
        has_internet,
    );
    let mut discovery_rx = discovery.start(shutdown_rx.clone()).await?;

    // X25519 keypair
    let (x25519_secret, x25519_public) = generate_x25519_keypair();

    let our_node_id = identity.node_id;
    let _our_display_name = identity.display_name.clone();
    let key_path = config.key_path.clone();
    let save_dir = config.key_path.parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mesh_received_files");
    let mut shutdown_rx2 = shutdown_rx.clone();

    let _ = event_tx.send(NodeEvent::Started {
        node_id: identity.node_id_hex(),
    }).await;

    let handle = NodeHandle {
        command_tx,
    };

    // Main event loop
    tokio::spawn(async move {
        let mut peers = PeerManager::new();
        let mut router = Router::new(our_node_id);
        let mut file_mgr = FileTransferManager::new(save_dir);
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut gateway_timer = tokio::time::interval(GATEWAY_CHECK_INTERVAL);
        let x25519_public_bytes = x25519_public.to_bytes();
        let mut known_gateways: HashSet<[u8; 32]> = HashSet::new();
        let mut active_call: Option<([u8; 32], [u8; 16])> = None; // (peer, stream_id)

        // Track write senders from inbound TCP connections
        let mut inbound_senders: HashMap<SocketAddr, mpsc::Sender<MeshMessage>> = HashMap::new();

        loop {
            tokio::select! {
                // ---------------------------------------------------------------
                // Commands from the application
                // ---------------------------------------------------------------
                Some(cmd) = command_rx.recv() => {
                    match cmd {
                        NodeCommand::SendBroadcast { text } => {
                            let msg = MeshMessage::text(our_node_id, &text);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::SendDirect { dest, text } => {
                            let msg = MeshMessage::text_to(our_node_id, dest, &text);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::SendFile { dest, file_path } => {
                            match file_mgr.prepare_send(dest, std::path::Path::new(&file_path)) {
                                Ok(metadata) => {
                                    let msg = MeshMessage::file_offer(our_node_id, dest, &metadata);
                                    for (_, sender) in peers.broadcast_senders() {
                                        let _ = sender.send(msg.clone()).await;
                                    }
                                    info!("File offer sent: {} ({} bytes, {} chunks)",
                                        metadata.filename, metadata.size_bytes, metadata.chunk_count);
                                }
                                Err(e) => {
                                    warn!("Failed to prepare file: {}", e);
                                }
                            }
                        }
                        NodeCommand::AcceptFile { file_id } => {
                            if let Some(sender_id) = file_mgr.accept_incoming(&file_id) {
                                let msg = MeshMessage::file_accept(our_node_id, sender_id, file_id);
                                for (_, sender) in peers.broadcast_senders() {
                                    let _ = sender.send(msg.clone()).await;
                                }
                                info!("Accepted file transfer {:?}", hex::encode(file_id));
                            }
                        }
                        NodeCommand::SendVoice { dest, audio_data, duration_ms } => {
                            let payload = VoiceNotePayload { duration_ms, audio_data };
                            let msg = MeshMessage::voice_note(our_node_id, dest, &payload);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::StartVoiceCall { peer } => {
                            let mut stream_id = [0u8; 16];
                            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut stream_id);
                            active_call = Some((peer, stream_id));
                            let ctrl = CallControlPayload { stream_id };
                            let msg = MeshMessage::call_start(our_node_id, peer, &ctrl);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::EndVoiceCall => {
                            if let Some((peer, stream_id)) = active_call.take() {
                                let ctrl = CallControlPayload { stream_id };
                                let msg = MeshMessage::call_end(our_node_id, peer, &ctrl);
                                for (_, sender) in peers.broadcast_senders() {
                                    let _ = sender.send(msg.clone()).await;
                                }
                            }
                        }
                        NodeCommand::SendAudioFrame { peer, data } => {
                            if let Some((_, stream_id)) = &active_call {
                                let seq = 0u32; // Sequence tracking could be added
                                let payload = VoiceStreamPayload {
                                    stream_id: *stream_id,
                                    sequence: seq,
                                    audio_frame: data,
                                };
                                let msg = MeshMessage::voice_stream(our_node_id, peer, &payload);
                                // Send directly to the call peer only
                                if let Some(p) = peers.get(&peer) {
                                    let _ = p.sender.send(msg).await;
                                }
                            }
                        }
                        NodeCommand::UpdateProfile { name, bio } => {
                            let payload = ProfilePayload {
                                display_name: name,
                                bio,
                                capabilities: vec!["text".into(), "voice".into(), "file".into()],
                            };
                            let msg = MeshMessage::profile_update(our_node_id, &payload);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::SendPublicBroadcast { text } => {
                            let msg = MeshMessage::public_broadcast(our_node_id, &text);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::SendSOS { text, location } => {
                            let payload = SOSPayload { text, location };
                            let msg = MeshMessage::sos(our_node_id, &payload);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::GetStats => {
                            let rs = &router.stats;
                            let stats = MeshStats {
                                total_peers: peers.count() as u32,
                                messages_relayed: rs.messages_relayed,
                                messages_received: rs.messages_received,
                                unique_nodes_seen: rs.unique_nodes_seen,
                                avg_hops: rs.avg_hops(),
                            };
                            let _ = event_tx.send(NodeEvent::Stats { stats }).await;
                        }
                        NodeCommand::GetPeers => {
                            let peer_list: Vec<PeerListEntry> = peers.all().map(|p| {
                                PeerListEntry {
                                    node_id: p.node_id,
                                    display_name: p.display_name.clone(),
                                    addr: p.addr.to_string(),
                                    is_gateway: p.is_gateway,
                                    bio: p.bio.clone(),
                                }
                            }).collect();
                            let _ = event_tx.send(NodeEvent::PeerList { peers: peer_list }).await;
                        }
                        NodeCommand::Nuke => {
                            info!("NUKE: Destroying identity and shutting down");
                            let _ = NodeIdentity::secure_delete(&key_path);
                            let _ = shutdown_tx.send(true);
                            let _ = event_tx.send(NodeEvent::Nuked).await;
                            break;
                        }
                        NodeCommand::Shutdown => {
                            info!("Graceful shutdown requested");
                            let _ = shutdown_tx.send(true);
                            let _ = event_tx.send(NodeEvent::Stopped).await;
                            break;
                        }
                    }
                }

                // ---------------------------------------------------------------
                // Track inbound TCP connections
                // ---------------------------------------------------------------
                Some(conn) = inbound_conn_rx.recv() => {
                    debug!("Inbound TCP connection from {}, storing write sender", conn.addr);
                    inbound_senders.insert(conn.addr, conn.sender);
                }

                // ---------------------------------------------------------------
                // Handle discovered peers
                // ---------------------------------------------------------------
                Some(discovered) = discovery_rx.recv() => {
                    if discovered.node_id == our_node_id || peers.contains(&discovered.node_id) {
                        if let Some(p) = peers.get_mut(&discovered.node_id) {
                            p.touch();
                            // Update gateway status from discovery
                            if discovered.has_internet && !p.is_gateway {
                                p.is_gateway = true;
                                let _ = event_tx.send(NodeEvent::GatewayFound {
                                    node_id: discovered.node_id,
                                    display_name: p.display_name.clone(),
                                }).await;
                                known_gateways.insert(discovered.node_id);
                            } else if !discovered.has_internet && p.is_gateway {
                                p.is_gateway = false;
                                let _ = event_tx.send(NodeEvent::GatewayLost {
                                    node_id: discovered.node_id,
                                }).await;
                                known_gateways.remove(&discovered.node_id);
                            }
                        }
                        continue;
                    }

                    info!("Connecting to discovered peer: {} at {}", discovered.display_name, discovered.addr);
                    match TcpTransport::connect_to_peer(discovered.addr, incoming_tx.clone()).await {
                        Ok((sender, _handle)) => {
                            let mut peer = PeerState::new(
                                discovered.node_id,
                                discovered.display_name.clone(),
                                discovered.addr,
                                sender.clone(),
                            );
                            peer.is_gateway = discovered.has_internet;
                            peers.add(peer);

                            let kx = KeyExchangePayload { x25519_public: x25519_public_bytes };
                            let kx_msg = kx.to_message(our_node_id, discovered.node_id);
                            let _ = sender.send(kx_msg).await;

                            let _ = event_tx.send(NodeEvent::PeerConnected {
                                node_id: discovered.node_id,
                                display_name: discovered.display_name.clone(),
                            }).await;

                            if discovered.has_internet {
                                let _ = event_tx.send(NodeEvent::GatewayFound {
                                    node_id: discovered.node_id,
                                    display_name: discovered.display_name,
                                }).await;
                                known_gateways.insert(discovered.node_id);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to connect to {}: {}", discovered.addr, e);
                        }
                    }
                }

                // ---------------------------------------------------------------
                // Handle incoming messages
                // ---------------------------------------------------------------
                Some(incoming) = incoming_rx.recv() => {
                    let msg = incoming.msg;
                    let from_addr = incoming.from_addr;

                    // --- Key Exchange (handled before routing) ---
                    if msg.msg_type == MessageType::KeyExchange {
                        if let Ok(kx) = KeyExchangePayload::from_message(&msg) {
                            let session = SessionKeys::from_exchange(&x25519_secret, &kx.x25519_public);
                            if let Some(peer) = peers.get_mut(&msg.sender_id) {
                                peer.session_keys = Some(session);
                                peer.touch();
                                debug!("Session keys established with {}", peer.display_name);
                            } else if let Some(sender) = inbound_senders.remove(&from_addr) {
                                let name = format!("node-{}", hex::encode(&msg.sender_id[..4]));
                                info!("Inbound peer registered: {} from {}", name, from_addr);
                                let mut peer = PeerState::new(
                                    msg.sender_id,
                                    name.clone(),
                                    from_addr,
                                    sender.clone(),
                                );
                                peer.session_keys = Some(session);
                                peers.add(peer);

                                let kx_resp = KeyExchangePayload { x25519_public: x25519_public_bytes };
                                let kx_msg = kx_resp.to_message(our_node_id, msg.sender_id);
                                let _ = sender.send(kx_msg).await;

                                let _ = event_tx.send(NodeEvent::PeerConnected {
                                    node_id: msg.sender_id,
                                    display_name: name,
                                }).await;
                            } else {
                                debug!("Key exchange from unknown peer {}", hex::encode(&msg.sender_id[..4]));
                            }
                        }
                        continue;
                    }

                    // --- Ping/Pong ---
                    if msg.msg_type == MessageType::Ping {
                        if let Some(peer) = peers.get_mut(&msg.sender_id) {
                            peer.touch();
                            let pong = MeshMessage::new(MessageType::Pong, our_node_id, 1, Some(msg.sender_id), vec![]);
                            let _ = peer.sender.send(pong).await;
                        }
                        continue;
                    }

                    if msg.msg_type == MessageType::Pong {
                        if let Some(peer) = peers.get_mut(&msg.sender_id) {
                            peer.touch();
                        }
                        continue;
                    }

                    // --- Routing: dedup, TTL check ---
                    if !router.should_process(&msg) {
                        continue;
                    }

                    // --- Process message if it's for us ---
                    if router.is_for_us(&msg) {
                        let sender_name = peers.get(&msg.sender_id)
                            .map(|p| p.display_name.clone())
                            .unwrap_or_else(|| hex::encode(&msg.sender_id[..4]));

                        match msg.msg_type {
                            MessageType::Text => {
                                let content = String::from_utf8_lossy(&msg.payload).to_string();
                                let _ = event_tx.send(NodeEvent::MessageReceived {
                                    sender_id: msg.sender_id,
                                    sender_name,
                                    content,
                                }).await;
                            }
                            MessageType::PublicBroadcast => {
                                let text = String::from_utf8_lossy(&msg.payload).to_string();
                                let _ = event_tx.send(NodeEvent::PublicBroadcast {
                                    sender_id: msg.sender_id,
                                    sender_name,
                                    text,
                                }).await;
                            }
                            MessageType::SOS => {
                                if let Ok(sos) = bincode::deserialize::<SOSPayload>(&msg.payload) {
                                    let _ = event_tx.send(NodeEvent::SOSReceived {
                                        sender_id: msg.sender_id,
                                        sender_name,
                                        text: sos.text,
                                        location: sos.location,
                                    }).await;
                                }
                            }
                            MessageType::ProfileUpdate => {
                                if let Ok(profile) = bincode::deserialize::<ProfilePayload>(&msg.payload) {
                                    if let Some(peer) = peers.get_mut(&msg.sender_id) {
                                        peer.display_name = profile.display_name.clone();
                                        peer.bio = profile.bio.clone();
                                        peer.capabilities = profile.capabilities.clone();
                                    }
                                    let _ = event_tx.send(NodeEvent::ProfileUpdated {
                                        node_id: msg.sender_id,
                                        name: profile.display_name,
                                        bio: profile.bio,
                                    }).await;
                                }
                            }
                            MessageType::FileOffer => {
                                if let Ok(offer) = bincode::deserialize::<FileOfferPayload>(&msg.payload) {
                                    file_mgr.register_incoming(offer.clone(), msg.sender_id);
                                    let _ = event_tx.send(NodeEvent::FileOffered {
                                        sender_id: msg.sender_id,
                                        sender_name,
                                        file_id: offer.file_id,
                                        filename: offer.filename,
                                        size: offer.size_bytes,
                                    }).await;
                                }
                            }
                            MessageType::FileAccept => {
                                if let Ok(accept) = bincode::deserialize::<FileAcceptPayload>(&msg.payload) {
                                    if file_mgr.mark_accepted(&accept.file_id) {
                                        // Send all chunks
                                        let dest = file_mgr.outgoing_dest(&accept.file_id).unwrap_or(msg.sender_id);
                                        while let Some(chunk_payload) = file_mgr.next_chunk(&accept.file_id) {
                                            let chunk_msg = MeshMessage::file_chunk(our_node_id, dest, &chunk_payload);
                                            for (_, sender) in peers.broadcast_senders() {
                                                let _ = sender.send(chunk_msg.clone()).await;
                                            }
                                        }
                                        file_mgr.remove_outgoing(&accept.file_id);
                                        info!("File transfer complete (sender side)");
                                    }
                                }
                            }
                            MessageType::FileChunk => {
                                if let Ok(chunk) = bincode::deserialize::<FileChunkPayload>(&msg.payload) {
                                    if let Some(pct) = file_mgr.receive_chunk(&chunk.file_id, chunk.sequence, chunk.data) {
                                        let _ = event_tx.send(NodeEvent::FileProgress {
                                            file_id: chunk.file_id,
                                            pct,
                                        }).await;

                                        if file_mgr.is_incoming_complete(&chunk.file_id) {
                                            match file_mgr.finalize_incoming(&chunk.file_id) {
                                                Ok(path) => {
                                                    let _ = event_tx.send(NodeEvent::FileComplete {
                                                        file_id: chunk.file_id,
                                                        path: path.to_string_lossy().to_string(),
                                                    }).await;
                                                    info!("File received: {}", path.display());
                                                }
                                                Err(e) => {
                                                    warn!("File finalization failed: {}", e);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            MessageType::Voice => {
                                if let Ok(voice) = bincode::deserialize::<VoiceNotePayload>(&msg.payload) {
                                    let _ = event_tx.send(NodeEvent::VoiceReceived {
                                        sender_id: msg.sender_id,
                                        sender_name,
                                        audio_data: voice.audio_data,
                                        duration_ms: voice.duration_ms,
                                    }).await;
                                }
                            }
                            MessageType::CallStart => {
                                if let Ok(ctrl) = bincode::deserialize::<CallControlPayload>(&msg.payload) {
                                    active_call = Some((msg.sender_id, ctrl.stream_id));
                                    let _ = event_tx.send(NodeEvent::IncomingCall {
                                        peer: msg.sender_id,
                                        peer_name: sender_name,
                                    }).await;
                                }
                            }
                            MessageType::CallEnd => {
                                if active_call.as_ref().map(|(p, _)| *p) == Some(msg.sender_id) {
                                    active_call = None;
                                }
                                let _ = event_tx.send(NodeEvent::CallEnded {
                                    peer: msg.sender_id,
                                }).await;
                            }
                            MessageType::VoiceStream => {
                                if let Ok(vs) = bincode::deserialize::<VoiceStreamPayload>(&msg.payload) {
                                    let _ = event_tx.send(NodeEvent::AudioFrame {
                                        peer: msg.sender_id,
                                        data: vs.audio_frame,
                                    }).await;
                                }
                            }
                            _ => {} // Discovery, Ping, Pong, PeerExchange handled above
                        }
                    }

                    // --- Forward to other peers ---
                    if router.should_forward(&msg) {
                        if let Some(forwarded) = router.prepare_forward(&msg) {
                            for (peer_id, sender) in peers.broadcast_senders() {
                                if peer_id != msg.sender_id {
                                    let _ = sender.send(forwarded.clone()).await;
                                }
                            }
                        }
                    }

                    // Touch sender
                    if let Some(peer) = peers.get_mut(&msg.sender_id) {
                        peer.touch();
                    }
                }

                // ---------------------------------------------------------------
                // Heartbeat
                // ---------------------------------------------------------------
                _ = heartbeat.tick() => {
                    for (_, sender) in peers.broadcast_senders() {
                        let ping = MeshMessage::new(MessageType::Ping, our_node_id, 1, None, vec![]);
                        let _ = sender.send(ping).await;
                    }

                    let stale = peers.prune_stale(PEER_TIMEOUT);
                    for id in &stale {
                        if known_gateways.remove(id) {
                            let _ = event_tx.send(NodeEvent::GatewayLost { node_id: *id }).await;
                        }
                        let _ = event_tx.send(NodeEvent::PeerDisconnected { node_id: *id }).await;
                    }

                    // Update peer count in stats
                    router.stats.total_peers = peers.count() as u32;

                    debug!("Heartbeat: {} peers connected, {} msgs seen", peers.count(), router.seen_count());
                }

                // ---------------------------------------------------------------
                // Gateway re-check
                // ---------------------------------------------------------------
                _ = gateway_timer.tick() => {
                    // Re-check our own internet status periodically
                    let _current = gateway::check_internet();
                    // Note: updating discovery payload would require restarting discovery service
                    // For now we just track peer gateways from their discovery broadcasts
                }

                // ---------------------------------------------------------------
                // Shutdown
                // ---------------------------------------------------------------
                _ = shutdown_rx2.changed() => {
                    info!("Node shutting down");
                    break;
                }
            }
        }

        drop(shutdown_tx);
    });

    Ok((identity, handle, event_rx))
}
