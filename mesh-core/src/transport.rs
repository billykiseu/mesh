use std::net::{SocketAddr, Ipv4Addr};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{info, debug, warn};
use anyhow::{Result, Context};

use crate::message::MeshMessage;

const TCP_PORT: u16 = 7332;

/// Read a length-prefixed message from a TCP stream.
/// Format: [4-byte big-endian length][message bytes]
pub async fn read_message(stream: &mut TcpStream) -> Result<Option<MeshMessage>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1_000_000 {
        anyhow::bail!("Message too large: {} bytes", len);
    }

    let mut msg_buf = vec![0u8; len];
    stream.read_exact(&mut msg_buf).await?;

    let msg = MeshMessage::from_bytes(&msg_buf)?;
    Ok(Some(msg))
}

/// Write a length-prefixed message to a TCP stream.
pub async fn write_message(stream: &mut TcpStream, msg: &MeshMessage) -> Result<()> {
    let frame = msg.to_frame();
    stream.write_all(&frame).await?;
    stream.flush().await?;
    Ok(())
}

/// An incoming message received from a TCP peer.
#[derive(Debug)]
pub struct IncomingMessage {
    pub msg: MeshMessage,
    pub from_addr: SocketAddr,
}

/// Notification when a new inbound TCP connection is established.
/// The node orchestrator uses this to register the peer and send messages back.
pub struct InboundConnection {
    pub addr: SocketAddr,
    pub sender: mpsc::Sender<MeshMessage>,
}

/// TCP transport listener + connection manager.
pub struct TcpTransport {
    listen_port: u16,
}

impl TcpTransport {
    pub fn new(listen_port: u16) -> Self {
        Self { listen_port }
    }

    pub fn default_port() -> u16 {
        TCP_PORT
    }

    /// Start listening for incoming TCP connections.
    pub async fn start_listener(
        &self,
        incoming_tx: mpsc::Sender<IncomingMessage>,
        inbound_conn_tx: mpsc::Sender<InboundConnection>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.listen_port);
        // Use SO_REUSEADDR so we can restart quickly without TIME_WAIT issues
        let listener = {
            use socket2::{Socket, Domain, Type, Protocol};
            let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
                .context("Failed to create TCP socket")?;
            socket.set_reuse_address(true)?;
            socket.set_nonblocking(true)?;
            let bind_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), self.listen_port);
            socket.bind(&bind_addr.into())
                .context(format!("Failed to bind TCP on port {}", self.listen_port))?;
            socket.listen(128)?;
            TcpListener::from_std(socket.into())?
        };
        info!("TCP transport listening on {}", addr);

        let mut shutdown_rx = shutdown;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                debug!("Incoming TCP connection from {}", addr);
                                let tx = incoming_tx.clone();
                                let conn_tx = inbound_conn_tx.clone();
                                tokio::spawn(handle_incoming_connection(stream, addr, tx, conn_tx));
                            }
                            Err(e) => {
                                warn!("TCP accept error: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("TCP listener shutting down");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Connect to a peer and start the read/write tasks.
    /// Returns a sender to write messages to the peer.
    pub async fn connect_to_peer(
        addr: SocketAddr,
        incoming_tx: mpsc::Sender<IncomingMessage>,
    ) -> Result<(mpsc::Sender<MeshMessage>, tokio::task::JoinHandle<()>)> {
        let stream = TcpStream::connect(addr).await?;
        debug!("Connected to peer at {}", addr);

        let (write_tx, write_rx) = mpsc::channel::<MeshMessage>(64);
        let handle = tokio::spawn(handle_peer_connection(stream, addr, incoming_tx, write_rx));

        Ok((write_tx, handle))
    }
}

/// Handle a bidirectional peer connection (used for both incoming and outgoing).
async fn handle_peer_connection(
    stream: TcpStream,
    addr: SocketAddr,
    incoming_tx: mpsc::Sender<IncomingMessage>,
    mut write_rx: mpsc::Receiver<MeshMessage>,
) {
    let (mut read_half, mut write_half) = stream.into_split();

    // Read task
    let tx = incoming_tx.clone();
    let read_task = tokio::spawn(async move {
        loop {
            let mut len_buf = [0u8; 4];
            match read_half.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::UnexpectedEof {
                        warn!("Peer {} read error: {}", addr, e);
                    }
                    break;
                }
            }

            let len = u32::from_be_bytes(len_buf) as usize;
            if len > 1_000_000 {
                warn!("Peer {} sent oversized message: {} bytes", addr, len);
                break;
            }

            let mut msg_buf = vec![0u8; len];
            if let Err(e) = read_half.read_exact(&mut msg_buf).await {
                warn!("Peer {} read payload error: {}", addr, e);
                break;
            }

            match MeshMessage::from_bytes(&msg_buf) {
                Ok(msg) => {
                    let _ = tx.send(IncomingMessage { msg, from_addr: addr }).await;
                }
                Err(e) => {
                    warn!("Peer {} invalid message: {}", addr, e);
                }
            }
        }
        debug!("Read task for {} ended", addr);
    });

    // Write task
    let write_task = tokio::spawn(async move {
        while let Some(msg) = write_rx.recv().await {
            let frame = msg.to_frame();
            if let Err(e) = write_half.write_all(&frame).await {
                warn!("Peer {} write error: {}", addr, e);
                break;
            }
            if let Err(e) = write_half.flush().await {
                warn!("Peer {} flush error: {}", addr, e);
                break;
            }
        }
        debug!("Write task for {} ended", addr);
    });

    // Wait for either task to finish
    tokio::select! {
        _ = read_task => {}
        _ = write_task => {}
    }
    debug!("Peer connection {} closed", addr);
}

/// Handle an incoming TCP connection: create a write channel and notify the
/// orchestrator so it can register the peer and send messages back.
async fn handle_incoming_connection(
    stream: TcpStream,
    addr: SocketAddr,
    incoming_tx: mpsc::Sender<IncomingMessage>,
    inbound_conn_tx: mpsc::Sender<InboundConnection>,
) {
    let (write_tx, write_rx) = mpsc::channel::<MeshMessage>(64);
    // Notify the orchestrator about this new inbound connection
    let _ = inbound_conn_tx.send(InboundConnection {
        addr,
        sender: write_tx,
    }).await;
    handle_peer_connection(stream, addr, incoming_tx, write_rx).await;
}
