use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn, debug};
use anyhow::Result;

use crate::crypto::{generate_x25519_keypair, SessionKeys};
use crate::discovery::DiscoveryService;
use crate::identity::NodeIdentity;
use crate::message::{MeshMessage, MessageType, KeyExchangePayload};
use crate::peer::{PeerManager, PeerState};
use crate::router::Router;
use crate::transport::{TcpTransport, IncomingMessage, InboundConnection};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const PEER_TIMEOUT: Duration = Duration::from_secs(30);
const TCP_PORT: u16 = 7332;

/// Events emitted by the node for the application layer.
#[derive(Debug, Clone)]
pub enum NodeEvent {
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
    Started {
        node_id: String,
    },
}

/// Configuration for the mesh node.
pub struct NodeConfig {
    pub display_name: String,
    pub listen_port: u16,
    pub key_path: std::path::PathBuf,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            display_name: "MeshNode".into(),
            listen_port: TCP_PORT,
            key_path: std::path::PathBuf::from("mesh_identity.key"),
        }
    }
}

/// A handle for sending messages from the application layer.
#[derive(Clone)]
pub struct NodeHandle {
    command_tx: mpsc::Sender<NodeCommand>,
}

pub enum NodeCommand {
    SendBroadcast { text: String },
    SendDirect { dest: [u8; 32], text: String },
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
}

/// Create and start a full mesh node, returning handles for the application.
pub async fn start_mesh_node(config: NodeConfig) -> Result<(NodeIdentity, NodeHandle, mpsc::Receiver<NodeEvent>)> {
    let identity = NodeIdentity::load_or_create(&config.key_path, config.display_name.clone())?;
    info!("Node identity: {} ({})", identity.node_id_short(), identity.display_name);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (event_tx, event_rx) = mpsc::channel::<NodeEvent>(256);
    let (command_tx, mut command_rx) = mpsc::channel::<NodeCommand>(256);
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<IncomingMessage>(256);
    let (inbound_conn_tx, mut inbound_conn_rx) = mpsc::channel::<InboundConnection>(64);

    // Start TCP listener
    let transport = TcpTransport::new(config.listen_port);
    transport.start_listener(incoming_tx.clone(), inbound_conn_tx, shutdown_rx.clone()).await?;

    // Start discovery
    let discovery = DiscoveryService::new(
        identity.node_id,
        identity.display_name.clone(),
        config.listen_port,
    );
    let mut discovery_rx = discovery.start(shutdown_rx.clone()).await?;

    // X25519 keypair
    let (x25519_secret, x25519_public) = generate_x25519_keypair();

    let our_node_id = identity.node_id;
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
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        let x25519_public_bytes = x25519_public.to_bytes();

        // Track write senders from inbound TCP connections, keyed by remote address.
        // When a KeyExchange arrives from an unknown peer, we look up the sender here.
        let mut inbound_senders: HashMap<SocketAddr, mpsc::Sender<MeshMessage>> = HashMap::new();

        loop {
            tokio::select! {
                // Commands from the application
                Some(cmd) = command_rx.recv() => {
                    match cmd {
                        NodeCommand::SendBroadcast { text } => {
                            let msg = MeshMessage::text(our_node_id, &text);
                            let senders = peers.broadcast_senders();
                            for (_, sender) in senders {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                        NodeCommand::SendDirect { dest, text } => {
                            let msg = MeshMessage::text_to(our_node_id, dest, &text);
                            for (_, sender) in peers.broadcast_senders() {
                                let _ = sender.send(msg.clone()).await;
                            }
                        }
                    }
                }

                // Track inbound TCP connections (write senders)
                Some(conn) = inbound_conn_rx.recv() => {
                    debug!("Inbound TCP connection from {}, storing write sender", conn.addr);
                    inbound_senders.insert(conn.addr, conn.sender);
                }

                // Handle discovered peers
                Some(discovered) = discovery_rx.recv() => {
                    if discovered.node_id == our_node_id || peers.contains(&discovered.node_id) {
                        if let Some(p) = peers.get_mut(&discovered.node_id) {
                            p.touch();
                        }
                        continue;
                    }

                    info!("Connecting to discovered peer: {} at {}", discovered.display_name, discovered.addr);
                    match TcpTransport::connect_to_peer(discovered.addr, incoming_tx.clone()).await {
                        Ok((sender, _handle)) => {
                            let peer = PeerState::new(
                                discovered.node_id,
                                discovered.display_name.clone(),
                                discovered.addr,
                                sender.clone(),
                            );
                            peers.add(peer);

                            let kx = KeyExchangePayload { x25519_public: x25519_public_bytes };
                            let kx_msg = kx.to_message(our_node_id, discovered.node_id);
                            let _ = sender.send(kx_msg).await;

                            let _ = event_tx.send(NodeEvent::PeerConnected {
                                node_id: discovered.node_id,
                                display_name: discovered.display_name,
                            }).await;
                        }
                        Err(e) => {
                            warn!("Failed to connect to {}: {}", discovered.addr, e);
                        }
                    }
                }

                // Handle incoming messages
                Some(incoming) = incoming_rx.recv() => {
                    let msg = incoming.msg;
                    let from_addr = incoming.from_addr;

                    if msg.msg_type == MessageType::KeyExchange {
                        if let Ok(kx) = KeyExchangePayload::from_message(&msg) {
                            let session = SessionKeys::from_exchange(&x25519_secret, &kx.x25519_public);
                            if let Some(peer) = peers.get_mut(&msg.sender_id) {
                                // Already know this peer - just update session keys
                                peer.session_keys = Some(session);
                                peer.touch();
                                debug!("Session keys established with {}", peer.display_name);
                            } else if let Some(sender) = inbound_senders.remove(&from_addr) {
                                // New peer connected to us - register using the inbound write sender
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

                                // Send our key exchange back
                                let kx_resp = KeyExchangePayload {
                                    x25519_public: x25519_public_bytes,
                                };
                                let kx_msg = kx_resp.to_message(our_node_id, msg.sender_id);
                                let _ = sender.send(kx_msg).await;

                                let _ = event_tx.send(NodeEvent::PeerConnected {
                                    node_id: msg.sender_id,
                                    display_name: name,
                                }).await;
                            } else {
                                debug!("Key exchange from unknown peer {} with no inbound sender", hex::encode(&msg.sender_id[..4]));
                            }
                        }
                        continue;
                    }

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

                    if !router.should_process(&msg) {
                        continue;
                    }

                    if router.is_for_us(&msg) && msg.msg_type == MessageType::Text {
                        let content = String::from_utf8_lossy(&msg.payload).to_string();
                        let sender_name = peers.get(&msg.sender_id)
                            .map(|p| p.display_name.clone())
                            .unwrap_or_else(|| hex::encode(&msg.sender_id[..4]));

                        let _ = event_tx.send(NodeEvent::MessageReceived {
                            sender_id: msg.sender_id,
                            sender_name,
                            content,
                        }).await;
                    }

                    if router.should_forward(&msg) {
                        if let Some(forwarded) = router.prepare_forward(&msg) {
                            for (peer_id, sender) in peers.broadcast_senders() {
                                if peer_id != msg.sender_id {
                                    let _ = sender.send(forwarded.clone()).await;
                                }
                            }
                        }
                    }

                    if let Some(peer) = peers.get_mut(&msg.sender_id) {
                        peer.touch();
                    }
                }

                // Heartbeat
                _ = heartbeat.tick() => {
                    for (_, sender) in peers.broadcast_senders() {
                        let ping = MeshMessage::new(MessageType::Ping, our_node_id, 1, None, vec![]);
                        let _ = sender.send(ping).await;
                    }

                    let stale = peers.prune_stale(PEER_TIMEOUT);
                    for id in stale {
                        let _ = event_tx.send(NodeEvent::PeerDisconnected { node_id: id }).await;
                    }

                    debug!("Heartbeat: {} peers connected, {} msgs seen", peers.count(), router.seen_count());
                }

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
