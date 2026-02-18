use std::net::{SocketAddr, Ipv4Addr};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, debug, warn};
use anyhow::{Result, Context};

use crate::message::{DiscoveryPayload, MeshMessage, MessageType};

const DISCOVERY_PORT: u16 = 7331;
const BROADCAST_ADDR: &str = "255.255.255.255";
const DISCOVERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Create a UDP socket with SO_REUSEADDR so multiple instances on the same
/// machine can share the discovery port.
fn create_reusable_udp_socket(port: u16) -> Result<std::net::UdpSocket> {
    use socket2::{Socket, Domain, Type, Protocol};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .context("Failed to create socket")?;
    socket.set_reuse_address(true)?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;

    let addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
    socket.bind(&addr.into())
        .context(format!("Failed to bind discovery socket on port {}", port))?;

    Ok(socket.into())
}

/// Event emitted when a peer is discovered.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    pub node_id: [u8; 32],
    pub display_name: String,
    pub addr: SocketAddr,
    pub listen_port: u16,
}

/// Runs the UDP broadcast discovery service.
pub struct DiscoveryService {
    our_node_id: [u8; 32],
    display_name: String,
    listen_port: u16,
}

impl DiscoveryService {
    pub fn new(node_id: [u8; 32], display_name: String, listen_port: u16) -> Self {
        Self {
            our_node_id: node_id,
            display_name,
            listen_port,
        }
    }

    /// Start both the broadcast sender and listener.
    /// Returns a receiver for discovered peers.
    pub async fn start(
        &self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<mpsc::Receiver<DiscoveredPeer>> {
        let (tx, rx) = mpsc::channel(64);

        // Bind the listener socket with SO_REUSEADDR for multi-instance support
        let std_socket = create_reusable_udp_socket(DISCOVERY_PORT)?;
        let listener = UdpSocket::from_std(std_socket)?;
        info!("Discovery listening on 0.0.0.0:{}", DISCOVERY_PORT);

        // Bind the sender on a different port (OS-assigned)
        let sender = UdpSocket::bind("0.0.0.0:0").await?;
        sender.set_broadcast(true)?;

        let our_node_id = self.our_node_id;
        let display_name = self.display_name.clone();
        let listen_port = self.listen_port;

        // Spawn the broadcast sender
        let mut shutdown_tx = shutdown.clone();
        tokio::spawn(async move {
            let payload = DiscoveryPayload::new(our_node_id, display_name, listen_port);
            let msg = payload.to_message();
            let data = msg.to_bytes();
            let broadcast_target = format!("{}:{}", BROADCAST_ADDR, DISCOVERY_PORT);

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(DISCOVERY_INTERVAL) => {
                        if let Err(e) = sender.send_to(&data, &broadcast_target).await {
                            warn!("Discovery broadcast failed: {}", e);
                        } else {
                            debug!("Sent discovery broadcast");
                        }
                    }
                    _ = shutdown_tx.changed() => {
                        info!("Discovery sender shutting down");
                        break;
                    }
                }
            }
        });

        // Spawn the listener
        let mut shutdown_rx = shutdown;
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                tokio::select! {
                    result = listener.recv_from(&mut buf) => {
                        match result {
                            Ok((len, src_addr)) => {
                                if let Ok(msg) = MeshMessage::from_bytes(&buf[..len]) {
                                    if msg.msg_type == MessageType::Discovery && msg.sender_id != our_node_id {
                                        if let Ok(payload) = DiscoveryPayload::from_message(&msg) {
                                            let peer = DiscoveredPeer {
                                                node_id: payload.node_id,
                                                display_name: payload.display_name,
                                                addr: SocketAddr::new(src_addr.ip(), payload.listen_port),
                                                listen_port: payload.listen_port,
                                            };
                                            debug!("Discovered peer: {} at {}", peer.display_name, peer.addr);
                                            let _ = tx.send(peer).await;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Discovery recv error: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("Discovery listener shutting down");
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}
