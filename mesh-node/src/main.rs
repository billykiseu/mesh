#![windows_subsystem = "windows"]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex, mpsc as std_mpsc};
use std::time::Instant;

use anyhow::Result;
use eframe::egui;
use egui::{Color32, CornerRadius, FontId, RichText, Stroke, StrokeKind, Vec2};
use egui_extras::{TableBuilder, Column};

use mesh_core::{NodeConfig, NodeEvent, NodeHandle, MeshStats, start_mesh_node, NodeIdentity};
use mesh_core::{TriagePayload, TriageLevel, ResourceRequestPayload, CheckInPayload};

// ---------------------------------------------------------------------------
// Colours (Discord-inspired dark theme)
// ---------------------------------------------------------------------------

const BG_DARK: Color32 = Color32::from_rgb(30, 31, 34);
const BG_SIDEBAR: Color32 = Color32::from_rgb(24, 25, 28);
const BG_INPUT: Color32 = Color32::from_rgb(43, 45, 49);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(220, 221, 222);
const TEXT_MUTED: Color32 = Color32::from_rgb(148, 155, 164);
const ACCENT_CYAN: Color32 = Color32::from_rgb(88, 201, 223);
const ACCENT_GREEN: Color32 = Color32::from_rgb(87, 242, 135);
const ACCENT_MAGENTA: Color32 = Color32::from_rgb(235, 69, 158);
const ACCENT_BLUE: Color32 = Color32::from_rgb(88, 101, 242);
const ACCENT_YELLOW: Color32 = Color32::from_rgb(254, 231, 92);
const ACCENT_RED: Color32 = Color32::from_rgb(237, 66, 69);

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

enum ChatMessageKind {
    System,
    Dm,
    Public,
    VoiceNote { audio_data: Vec<u8>, duration_ms: u32 },
}

struct ChatMessage {
    sender: String,
    content: String,
    kind: ChatMessageKind,
}

struct PeerEntry {
    node_id: [u8; 32],
    display_name: String,
    is_gateway: bool,
    bio: String,
}

struct FileEntry {
    file_id: [u8; 16],
    filename: String,
    size: u64,
    progress: u8,
    done: bool,
    incoming: bool,
    path: Option<String>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Chat,
    Peers,
    Files,
    Emergency,
    Settings,
}

// ---------------------------------------------------------------------------
// MeshApp state
// ---------------------------------------------------------------------------

struct MeshApp {
    // Identity
    display_name: String,
    node_id_hex: String,
    node_id_short: String,
    port: u16,

    // Data
    messages: Vec<ChatMessage>,
    peers: Vec<PeerEntry>,
    files: Vec<FileEntry>,
    stats: MeshStats,
    gateway_name: Option<String>,

    // UI state
    input: String,
    active_tab: Tab,
    dm_target: Option<([u8; 32], String)>,
    show_nuke_confirm: bool,
    pending_file_offer: Option<([u8; 16], String, String, u64)>,
    should_quit: bool,

    // Settings tab state
    settings_name: String,
    bio: String,

    // Voice note state
    recording: bool,
    record_start: Option<Instant>,
    audio_buffer: Arc<StdMutex<Vec<i16>>>,
    audio_input_stream: Option<cpal::Stream>,
    playback_active: bool,
    audio_output_stream: Option<cpal::Stream>,

    // Voice call state
    in_call: Option<([u8; 32], String)>,
    incoming_call: Option<([u8; 32], String)>,
    call_input_stream: Option<cpal::Stream>,
    call_output_stream: Option<cpal::Stream>,
    call_playback_buffer: Arc<StdMutex<VecDeque<i16>>>,

    // File send result channel
    file_pick_rx: Option<std_mpsc::Receiver<std::path::PathBuf>>,

    // Groups
    joined_groups: Vec<String>,
    active_group: Option<String>,
    group_join_input: String,

    // Typing indicators
    typing_peers: Vec<([u8; 32], String, Instant)>,
    _typing_sent: bool,
    _last_typed: Option<Instant>,

    // Emergency data
    triage_log: Vec<(String, TriagePayload)>,
    resource_log: Vec<(String, ResourceRequestPayload)>,
    safety_roster: Vec<([u8; 32], String, String, Instant)>, // (peer, name, status, time)

    // Disappearing messages
    disappearing_msgs: Vec<(String, String, Instant, u32)>, // (sender, text, received_at, ttl)

    // Check-in button / triage / resource input
    triage_victim_id: String,
    triage_notes: String,
    triage_level: String,
    resource_category: String,
    resource_description: String,
    resource_urgency: String,

    // Notification muted
    notifications_muted: bool,

    // Node identity (for safety numbers)
    our_node_id: [u8; 32],

    // Async bridge
    handle: NodeHandle,
    rt: Arc<tokio::runtime::Runtime>,
    bridge_rx: std_mpsc::Receiver<NodeEvent>,
}

impl MeshApp {
    fn push_system(&mut self, msg: String) {
        self.messages.push(ChatMessage {
            sender: String::new(),
            content: msg,
            kind: ChatMessageKind::System,
        });
    }

    fn push_chat(&mut self, sender: String, content: String, is_dm: bool) {
        self.messages.push(ChatMessage {
            sender,
            content,
            kind: if is_dm { ChatMessageKind::Dm } else { ChatMessageKind::Public },
        });
    }

    fn push_voice_note(&mut self, sender: String, audio_data: Vec<u8>, duration_ms: u32) {
        let secs = duration_ms as f64 / 1000.0;
        self.messages.push(ChatMessage {
            sender: sender.clone(),
            content: format!("Voice note ({:.1}s)", secs),
            kind: ChatMessageKind::VoiceNote { audio_data, duration_ms },
        });
    }

    fn handle_mesh_event(&mut self, event: NodeEvent) {
        match event {
            NodeEvent::Started { node_id } => {
                self.push_system(format!("Node started ({})", &node_id[..8]));
            }
            NodeEvent::PeerConnected { node_id, display_name } => {
                self.push_system(format!("+ {} connected", display_name));
                self.peers.push(PeerEntry {
                    node_id,
                    display_name,
                    is_gateway: false,
                    bio: String::new(),
                });
            }
            NodeEvent::PeerDisconnected { node_id } => {
                if let Some(idx) = self.peers.iter().position(|p| p.node_id == node_id) {
                    let name = self.peers.remove(idx).display_name;
                    self.push_system(format!("- {} disconnected", name));
                } else {
                    self.push_system(format!("- {} disconnected", hex::encode(&node_id[..4])));
                }
                // End call if the peer disconnected
                if self.in_call.as_ref().map(|(id, _)| *id) == Some(node_id) {
                    self.end_call();
                }
            }
            NodeEvent::MessageReceived { sender_name, content, .. } => {
                self.push_chat(sender_name, content, true);
            }
            NodeEvent::PublicBroadcast { sender_name, text, .. } => {
                self.push_chat(format!("[PUBLIC] {}", sender_name), text, false);
            }
            NodeEvent::SOSReceived { sender_name, text, location, .. } => {
                let loc_str = location
                    .map(|(lat, lon)| format!(" @ {:.4},{:.4}", lat, lon))
                    .unwrap_or_default();
                self.push_system(format!("!!! SOS from {}: {}{}", sender_name, text, loc_str));
            }
            NodeEvent::ProfileUpdated { node_id, name, bio } => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.node_id == node_id) {
                    peer.display_name = name.clone();
                    peer.bio = bio;
                }
                self.push_system(format!("* {} updated their profile", name));
            }
            NodeEvent::FileOffered { sender_name, file_id, filename, size, .. } => {
                self.files.push(FileEntry {
                    file_id,
                    filename: filename.clone(),
                    size,
                    progress: 0,
                    done: false,
                    incoming: true,
                    path: None,
                });
                let size_str = format_size(size);
                self.push_system(format!(
                    "File offered by {}: {} ({})",
                    sender_name, filename, size_str
                ));
                self.pending_file_offer = Some((file_id, sender_name, filename, size));
            }
            NodeEvent::FileProgress { file_id, pct } => {
                if let Some(f) = self.files.iter_mut().find(|f| f.file_id == file_id) {
                    f.progress = pct;
                }
            }
            NodeEvent::FileComplete { file_id, path } => {
                if let Some(f) = self.files.iter_mut().find(|f| f.file_id == file_id) {
                    f.done = true;
                    f.progress = 100;
                    f.path = Some(path.clone());
                }
                self.push_system(format!("File received: {}", path));
            }
            NodeEvent::VoiceReceived { sender_name, audio_data, duration_ms, .. } => {
                self.push_voice_note(sender_name, audio_data, duration_ms);
            }
            NodeEvent::IncomingCall { peer, peer_name } => {
                self.push_system(format!("Incoming call from {}", peer_name));
                self.incoming_call = Some((peer, peer_name));
            }
            NodeEvent::AudioFrame { peer, data } => {
                if self.in_call.as_ref().map(|(id, _)| *id) == Some(peer) {
                    // Convert bytes to i16 samples and push to playback buffer
                    let samples: Vec<i16> = data.chunks_exact(2)
                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    if let Ok(mut buf) = self.call_playback_buffer.lock() {
                        buf.extend(samples);
                        // Cap buffer at 2 seconds (16000 * 2 = 32000 samples)
                        while buf.len() > 32000 {
                            buf.pop_front();
                        }
                    }
                }
            }
            NodeEvent::CallEnded { peer } => {
                if self.in_call.as_ref().map(|(id, _)| *id) == Some(peer) {
                    self.end_call();
                    self.push_system("Call ended".into());
                }
            }
            NodeEvent::GatewayFound { display_name, .. } => {
                self.gateway_name = Some(display_name.clone());
                self.push_system(format!("Gateway found: {}", display_name));
            }
            NodeEvent::GatewayLost { .. } => {
                self.gateway_name = None;
                self.push_system("Gateway lost".into());
            }
            NodeEvent::Stats { stats } => {
                self.stats = stats;
            }
            NodeEvent::PeerList { .. } => {}
            NodeEvent::MessageDelivered { .. } => {
                // Read receipt received - could update UI message status
            }
            NodeEvent::TypingStarted { peer, peer_name } => {
                if !self.typing_peers.iter().any(|(id, _, _)| *id == peer) {
                    self.typing_peers.push((peer, peer_name, Instant::now()));
                }
            }
            NodeEvent::TypingStopped { peer } => {
                self.typing_peers.retain(|(id, _, _)| *id != peer);
            }
            NodeEvent::GroupMessageReceived { group, sender_name, content, .. } => {
                self.push_chat(format!("[{}] {}", group, sender_name), content, false);
            }
            NodeEvent::GroupJoined { group, peer_name, .. } => {
                self.push_system(format!("{} joined group {}", peer_name, group));
            }
            NodeEvent::GroupLeft { group, .. } => {
                self.push_system(format!("Peer left group {}", group));
            }
            NodeEvent::TriageReceived { sender_name, triage, .. } => {
                self.push_system(format!(
                    "TRIAGE [{}] from {}: victim={} - {}",
                    triage.level.label(), sender_name, triage.victim_id, triage.notes
                ));
                self.triage_log.push((sender_name, triage));
            }
            NodeEvent::ResourceRequestReceived { sender_name, request, .. } => {
                self.push_system(format!(
                    "RESOURCE REQ from {}: [{}] urgency={} - {}",
                    sender_name, request.category, request.urgency, request.description
                ));
                self.resource_log.push((sender_name, request));
            }
            NodeEvent::CheckInReceived { sender_id, sender_name, check_in, .. } => {
                let status_str = format!("{}: {}", check_in.status, check_in.message);
                self.push_system(format!("CHECK-IN from {}: {}", sender_name, status_str));
                // Update safety roster
                self.safety_roster.retain(|(id, _, _, _)| *id != sender_id);
                self.safety_roster.push((sender_id, sender_name, check_in.status, Instant::now()));
            }
            NodeEvent::DisappearingReceived { sender_name, text, ttl_seconds, .. } => {
                self.push_chat(format!("[DISAPPEARING] {}", sender_name), text.clone(), false);
                self.disappearing_msgs.push((sender_name, text, Instant::now(), ttl_seconds));
            }
            NodeEvent::HistoryLoaded { .. } => {
                // History messages could be loaded into the chat view
            }
            NodeEvent::Nuked => {
                self.push_system("Identity destroyed. Shutting down.".into());
                self.should_quit = true;
            }
            NodeEvent::Stopped => {
                self.push_system("Node stopped.".into());
                self.should_quit = true;
            }
        }
    }

    fn spawn_cmd<F>(&self, f: F)
    where
        F: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.rt.spawn(async move {
            if let Err(e) = f.await {
                tracing::error!("Command failed: {}", e);
            }
        });
    }

    fn handle_input(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.clear();

        if text.starts_with('/') {
            self.handle_slash_command(&text);
        } else if let Some(group) = &self.active_group {
            let group = group.clone();
            self.push_chat(format!("[{}] You", group), text.clone(), false);
            let h = self.handle.clone();
            self.spawn_cmd(async move { h.send_group_message(&group, &text).await });
        } else if let Some((dest, name)) = &self.dm_target {
            let dest = *dest;
            let name = name.clone();
            self.push_chat(format!("DM to {}", name), text.clone(), true);
            let h = self.handle.clone();
            self.spawn_cmd(async move { h.send_direct(dest, &text).await });
        } else {
            self.push_chat("You".into(), text.clone(), false);
            let h = self.handle.clone();
            self.spawn_cmd(async move { h.send_broadcast(&text).await });
        }
    }

    fn handle_slash_command(&mut self, text: &str) {
        let parts: Vec<&str> = text.splitn(3, ' ').collect();
        let cmd = parts[0].to_lowercase();

        match cmd.as_str() {
            "/dm" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let msg = parts[2].to_string();
                    if let Some(peer) = self.peers.iter().find(|p| {
                        p.display_name.to_lowercase() == name.to_lowercase()
                            || hex::encode(&p.node_id[..4]) == name
                    }) {
                        let dest = peer.node_id;
                        let peer_name = peer.display_name.clone();
                        self.push_chat(format!("DM to {}", peer_name), msg.clone(), true);
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.send_direct(dest, &msg).await });
                    } else {
                        self.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    self.push_system("Usage: /dm <name> <message>".into());
                }
            }
            "/send" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let path = parts[2].to_string();
                    if let Some(peer) = self.peers.iter().find(|p| {
                        p.display_name.to_lowercase() == name.to_lowercase()
                            || hex::encode(&p.node_id[..4]) == name
                    }) {
                        let dest = peer.node_id;
                        self.push_system(format!("Sending file: {}", path));
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.send_file(dest, &path).await });
                    } else {
                        self.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    self.push_system("Usage: /send <peer> <filepath>".into());
                }
            }
            "/accept" => {
                if let Some(f) = self.files.iter().rev().find(|f| f.incoming && !f.done && f.progress == 0) {
                    let file_id = f.file_id;
                    let h = self.handle.clone();
                    self.push_system("File transfer accepted".into());
                    self.spawn_cmd(async move { h.accept_file(file_id).await });
                } else {
                    self.push_system("No pending file offers".into());
                }
            }
            "/name" => {
                if parts.len() >= 2 {
                    let new_name = parts[1..].join(" ");
                    self.display_name = new_name.clone();
                    self.settings_name = new_name.clone();
                    self.push_system(format!("Name changed to: {}", new_name));
                    let h = self.handle.clone();
                    let bio = self.bio.clone();
                    self.spawn_cmd(async move { h.update_profile(&new_name, &bio).await });
                } else {
                    self.push_system("Usage: /name <new_name>".into());
                }
            }
            "/broadcast" => {
                if parts.len() >= 2 {
                    let msg = text.strip_prefix("/broadcast ").unwrap_or("").to_string();
                    self.push_chat("[PUBLIC] You".into(), msg.clone(), false);
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_public_broadcast(&msg).await });
                } else {
                    self.push_system("Usage: /broadcast <message>".into());
                }
            }
            "/sos" => {
                if parts.len() >= 2 {
                    let msg = text.strip_prefix("/sos ").unwrap_or("").to_string();
                    self.push_system(format!("!!! SOS sent: {}", msg));
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_sos(&msg, None).await });
                } else {
                    self.push_system("Usage: /sos <message>".into());
                }
            }
            "/stats" => {
                let h = self.handle.clone();
                self.spawn_cmd(async move { h.get_stats().await });
                self.active_tab = Tab::Settings;
            }
            "/peers" => {
                self.active_tab = Tab::Peers;
            }
            "/nuke" => {
                self.show_nuke_confirm = true;
            }
            "/voice" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let path = parts[2].to_string();
                    if let Some(peer) = self.peers.iter().find(|p| {
                        p.display_name.to_lowercase() == name.to_lowercase()
                    }) {
                        let dest = peer.node_id;
                        match std::fs::read(&path) {
                            Ok(data) => {
                                let duration = (data.len() as u32 * 8) / 16;
                                let h = self.handle.clone();
                                self.push_system(format!("Voice note sent to {}", name));
                                self.spawn_cmd(async move {
                                    h.send_voice(Some(dest), data, duration).await
                                });
                            }
                            Err(e) => self.push_system(format!("Failed to read file: {}", e)),
                        }
                    } else {
                        self.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    self.push_system("Usage: /voice <peer> <filepath.opus>".into());
                }
            }
            "/call" => {
                if parts.len() >= 2 {
                    let name = parts[1];
                    if let Some(peer) = self.peers.iter().find(|p| {
                        p.display_name.to_lowercase() == name.to_lowercase()
                    }) {
                        let dest = peer.node_id;
                        let peer_name = peer.display_name.clone();
                        self.start_call(dest, peer_name);
                    } else {
                        self.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    self.push_system("Usage: /call <peer>".into());
                }
            }
            "/endcall" => {
                self.end_call();
            }
            "/group" => {
                if parts.len() >= 3 {
                    let subcmd = parts[1].to_lowercase();
                    let group_name = parts[2].to_string();
                    match subcmd.as_str() {
                        "join" => {
                            self.joined_groups.push(group_name.clone());
                            self.push_system(format!("Joining group: {}", group_name));
                            let h = self.handle.clone();
                            self.spawn_cmd(async move { h.join_group(&group_name).await });
                        }
                        "leave" => {
                            self.joined_groups.retain(|g| g != &group_name);
                            if self.active_group.as_deref() == Some(&group_name) {
                                self.active_group = None;
                            }
                            self.push_system(format!("Leaving group: {}", group_name));
                            let h = self.handle.clone();
                            self.spawn_cmd(async move { h.leave_group(&group_name).await });
                        }
                        _ => self.push_system("Usage: /group join|leave <name>".into()),
                    }
                } else {
                    self.push_system("Usage: /group join|leave <name>".into());
                }
            }
            "/triage" => {
                // /triage <level> <victim-id> <notes>
                if parts.len() >= 3 {
                    let level_str = parts[1];
                    let rest = text.strip_prefix("/triage ").unwrap_or("")
                        .strip_prefix(level_str).unwrap_or("").trim();
                    let (victim, notes) = rest.split_once(' ').unwrap_or((rest, ""));
                    if let Some(level) = TriageLevel::from_str(level_str) {
                        let payload = TriagePayload {
                            level,
                            victim_id: victim.to_string(),
                            notes: notes.to_string(),
                            location: None,
                        };
                        self.push_system(format!("TRIAGE [{}] victim={}: {}", payload.level.label(), victim, notes));
                        self.triage_log.push(("You".into(), payload.clone()));
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.send_triage(payload).await });
                    } else {
                        self.push_system("Invalid triage level. Use: black, red, yellow, green".into());
                    }
                } else {
                    self.push_system("Usage: /triage <red|yellow|green|black> <victim-id> <notes>".into());
                }
            }
            "/resource" => {
                // /resource <category> <urgency> <description>
                if parts.len() >= 3 {
                    let cat = parts[1].to_string();
                    let rest = text.strip_prefix("/resource ").unwrap_or("")
                        .strip_prefix(&cat).unwrap_or("").trim();
                    let (urgency_str, desc) = rest.split_once(' ').unwrap_or((rest, ""));
                    let urgency: u8 = urgency_str.parse().unwrap_or(3);
                    let payload = ResourceRequestPayload {
                        category: cat, description: desc.to_string(), urgency, location: None, quantity: 1,
                    };
                    self.push_system(format!("RESOURCE REQ [{}] urgency={}: {}", payload.category, urgency, desc));
                    self.resource_log.push(("You".into(), payload.clone()));
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_resource_request(payload).await });
                } else {
                    self.push_system("Usage: /resource <category> <urgency> <description>".into());
                }
            }
            "/checkin" => {
                let status = if parts.len() >= 2 { parts[1] } else { "ok" };
                let msg = if parts.len() >= 3 {
                    text.strip_prefix("/checkin ").unwrap_or("")
                        .strip_prefix(status).unwrap_or("").trim().to_string()
                } else { String::new() };
                let payload = CheckInPayload {
                    status: status.to_string(), location: None, message: msg,
                };
                self.push_system(format!("CHECK-IN: {}", status));
                let h = self.handle.clone();
                self.spawn_cmd(async move { h.send_check_in(payload).await });
            }
            "/disappear" => {
                // /disappear <seconds> <message>
                if parts.len() >= 3 {
                    let ttl: u32 = parts[1].parse().unwrap_or(30);
                    let msg = text.strip_prefix("/disappear ").unwrap_or("")
                        .strip_prefix(parts[1]).unwrap_or("").trim().to_string();
                    let dest = self.dm_target.as_ref().map(|(id, _)| *id);
                    self.push_system(format!("Disappearing ({}s): {}", ttl, msg));
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_disappearing(dest, &msg, ttl).await });
                } else {
                    self.push_system("Usage: /disappear <seconds> <message>".into());
                }
            }
            "/help" => {
                self.push_system("Commands:".into());
                self.push_system("  /dm <name> <msg>       - Direct message".into());
                self.push_system("  /send <peer> <path>    - Send file".into());
                self.push_system("  /accept                - Accept file offer".into());
                self.push_system("  /voice <peer> <path>   - Send voice file".into());
                self.push_system("  /call <peer>           - Start voice call".into());
                self.push_system("  /endcall               - End voice call".into());
                self.push_system("  /broadcast <msg>       - Public broadcast".into());
                self.push_system("  /sos <msg>             - Emergency broadcast".into());
                self.push_system("  /name <name>           - Change display name".into());
                self.push_system("  /group join|leave <n>  - Group management".into());
                self.push_system("  /triage <lvl> <id> <n> - Triage tag".into());
                self.push_system("  /resource <cat> <u> <d>- Resource request".into());
                self.push_system("  /checkin [status] [msg]- Safety check-in".into());
                self.push_system("  /disappear <s> <msg>   - Disappearing message".into());
                self.push_system("  /stats                 - Show mesh stats".into());
                self.push_system("  /peers                 - Show peer list".into());
                self.push_system("  /nuke                  - Destroy identity & exit".into());
            }
            _ => {
                self.push_system(format!("Unknown command: {}. Type /help", cmd));
            }
        }
    }

    // --- Voice note recording ---

    fn start_recording(&mut self) {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                self.push_system("No audio input device found".into());
                return;
            }
        };

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer = self.audio_buffer.clone();
        // Clear previous recording
        if let Ok(mut b) = buffer.lock() {
            b.clear();
        }

        let stream = match device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buffer.lock() {
                    b.extend_from_slice(data);
                }
            },
            |err| {
                tracing::error!("Audio input error: {}", err);
            },
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.push_system(format!("Failed to start recording: {}", e));
                return;
            }
        };

        if let Err(e) = stream.play() {
            self.push_system(format!("Failed to play input stream: {}", e));
            return;
        }

        self.audio_input_stream = Some(stream);
        self.recording = true;
        self.record_start = Some(Instant::now());
    }

    fn stop_recording_and_send(&mut self) {
        self.recording = false;
        // Drop the input stream to stop recording
        self.audio_input_stream = None;

        let samples = if let Ok(b) = self.audio_buffer.lock() {
            b.clone()
        } else {
            return;
        };

        if samples.is_empty() {
            self.push_system("No audio recorded".into());
            return;
        }

        // Convert i16 samples to bytes (little-endian)
        let audio_data: Vec<u8> = samples.iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();

        let duration_ms = (samples.len() as u32 * 1000) / 16000;
        let secs = duration_ms as f64 / 1000.0;

        let dest = self.dm_target.as_ref().map(|(id, _)| *id);
        let dest_name = self.dm_target.as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "all".into());

        self.push_system(format!("Voice note sent to {} ({:.1}s)", dest_name, secs));

        let h = self.handle.clone();
        self.spawn_cmd(async move {
            h.send_voice(dest, audio_data, duration_ms).await
        });
    }

    fn play_voice_note(&mut self, audio_data: &[u8], _duration_ms: u32) {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => {
                self.push_system("No audio output device found".into());
                return;
            }
        };

        // Use device's preferred config instead of forcing 16kHz mono
        let supported_config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                self.push_system(format!("No supported audio config: {}", e));
                return;
            }
        };
        let device_rate = supported_config.sample_rate().0;
        let device_channels = supported_config.channels() as usize;

        let config = cpal::StreamConfig {
            channels: device_channels as u16,
            sample_rate: cpal::SampleRate(device_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Convert bytes to 16kHz mono i16 samples
        let mono_16k: Vec<i16> = audio_data.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        // Resample from 16kHz to device rate and expand to device channels
        let resampled = resample_audio(&mono_16k, 16000, device_rate, device_channels);

        let samples = Arc::new(StdMutex::new((resampled, 0usize)));
        let samples_clone = samples.clone();

        let stream = match device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                if let Ok(mut s) = samples_clone.lock() {
                    let (ref buf, ref mut pos) = *s;
                    for sample in data.iter_mut() {
                        if *pos < buf.len() {
                            *sample = buf[*pos];
                            *pos += 1;
                        } else {
                            *sample = 0.0;
                        }
                    }
                }
            },
            |err| {
                tracing::error!("Audio output error: {}", err);
            },
            None,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.push_system(format!("Failed to play audio: {}", e));
                return;
            }
        };

        if let Err(e) = stream.play() {
            self.push_system(format!("Failed to start playback: {}", e));
            return;
        }

        self.playback_active = true;
        self.audio_output_stream = Some(stream);
    }

    // --- Voice call ---

    fn start_call(&mut self, peer_id: [u8; 32], peer_name: String) {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        if self.in_call.is_some() {
            self.push_system("Already in a call".into());
            return;
        }

        let host = cpal::default_host();

        // Input: use device preferred config, downsample to 16kHz mono for network
        let handle = self.handle.clone();
        let rt = self.rt.clone();

        let input_stream = if let Some(input_device) = host.default_input_device() {
            let in_cfg = input_device.default_input_config().ok();
            if let Some(supported) = in_cfg {
                let in_rate = supported.sample_rate().0;
                let in_channels = supported.channels() as usize;
                let config = cpal::StreamConfig {
                    channels: in_channels as u16,
                    sample_rate: cpal::SampleRate(in_rate),
                    buffer_size: cpal::BufferSize::Default,
                };
                // 20ms frame at 16kHz = 320 samples; at device rate = in_rate/50
                let frame_size_16k = 320usize;
                let downsample_buf = Arc::new(StdMutex::new(Vec::<f32>::new()));
                let downsample_buf_clone = downsample_buf.clone();

                match input_device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut fb) = downsample_buf_clone.lock() {
                            // Mix to mono
                            for chunk in data.chunks(in_channels) {
                                let mono: f32 = chunk.iter().sum::<f32>() / in_channels as f32;
                                fb.push(mono);
                            }
                            // Downsample to 16kHz in frame_size_16k chunks
                            let samples_per_frame = (in_rate as usize) / 50; // 20ms
                            while fb.len() >= samples_per_frame {
                                let device_frame: Vec<f32> = fb.drain(..samples_per_frame).collect();
                                let ratio = 16000.0 / in_rate as f64;
                                let mut frame_16k = Vec::with_capacity(frame_size_16k);
                                for i in 0..frame_size_16k {
                                    let src_idx = (i as f64 / ratio).min((device_frame.len() - 1) as f64);
                                    let idx = src_idx as usize;
                                    let frac = src_idx - idx as f64;
                                    let s0 = device_frame[idx];
                                    let s1 = device_frame[(idx + 1).min(device_frame.len() - 1)];
                                    let sample = s0 + (s1 - s0) * frac as f32;
                                    frame_16k.push(sample);
                                }
                                let frame_bytes: Vec<u8> = frame_16k.iter()
                                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                                    .flat_map(|s| s.to_le_bytes())
                                    .collect();
                                let h = handle.clone();
                                let pid = peer_id;
                                rt.spawn(async move {
                                    let _ = h.send_audio_frame(pid, frame_bytes).await;
                                });
                            }
                        }
                    },
                    |err| { tracing::error!("Call input error: {}", err); },
                    None,
                ) {
                    Ok(s) => { let _ = s.play(); Some(s) }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        // Output: use device preferred config, upsample from 16kHz mono
        let playback_buf = self.call_playback_buffer.clone();
        let output_stream = if let Some(output_device) = host.default_output_device() {
            let out_cfg = output_device.default_output_config().ok();
            if let Some(supported) = out_cfg {
                let out_rate = supported.sample_rate().0;
                let out_channels = supported.channels() as usize;
                let config = cpal::StreamConfig {
                    channels: out_channels as u16,
                    sample_rate: cpal::SampleRate(out_rate),
                    buffer_size: cpal::BufferSize::Default,
                };
                let resample_state = Arc::new(StdMutex::new(0.0f64)); // fractional position

                match output_device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        if let Ok(mut buf) = playback_buf.lock() {
                            let ratio = 16000.0 / out_rate as f64;
                            if let Ok(mut frac_pos) = resample_state.lock() {
                                for frame in data.chunks_mut(out_channels) {
                                    let sample = if !buf.is_empty() {
                                        let idx = (*frac_pos as usize).min(buf.len().saturating_sub(1));
                                        let s = buf[idx] as f32 / 32768.0;
                                        *frac_pos += ratio;
                                        while *frac_pos >= 1.0 && !buf.is_empty() {
                                            buf.pop_front();
                                            *frac_pos -= 1.0;
                                        }
                                        s
                                    } else {
                                        0.0
                                    };
                                    for ch in frame.iter_mut() {
                                        *ch = sample;
                                    }
                                }
                            }
                        }
                    },
                    |err| { tracing::error!("Call output error: {}", err); },
                    None,
                ) {
                    Ok(s) => { let _ = s.play(); Some(s) }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        // Send call start command
        let h = self.handle.clone();
        let pid = peer_id;
        self.spawn_cmd(async move { h.start_voice_call(pid).await });

        self.in_call = Some((peer_id, peer_name.clone()));
        self.call_input_stream = input_stream;
        self.call_output_stream = output_stream;
        self.push_system(format!("Calling {}...", peer_name));
    }

    fn accept_call(&mut self) {
        if let Some((peer_id, peer_name)) = self.incoming_call.take() {
            self.start_call(peer_id, peer_name);
        }
    }

    fn decline_call(&mut self) {
        if let Some((_, peer_name)) = self.incoming_call.take() {
            self.push_system(format!("Declined call from {}", peer_name));
        }
    }

    fn end_call(&mut self) {
        if let Some((_, ref name)) = self.in_call {
            self.push_system(format!("Call with {} ended", name));
        }
        self.in_call = None;
        self.call_input_stream = None;
        self.call_output_stream = None;
        if let Ok(mut buf) = self.call_playback_buffer.lock() {
            buf.clear();
        }
        let h = self.handle.clone();
        self.spawn_cmd(async move { h.end_voice_call().await });
    }

    // --- File send via picker ---

    fn open_file_picker(&mut self) {
        let (tx, rx) = std_mpsc::channel();
        self.file_pick_rx = Some(rx);

        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let _ = tx.send(path);
            }
        });
    }

    fn check_file_pick_result(&mut self) {
        if let Some(ref rx) = self.file_pick_rx {
            if let Ok(path) = rx.try_recv() {
                self.file_pick_rx = None;
                let path_str = path.display().to_string();
                let filename = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path_str.clone());

                if let Some((dest, name)) = &self.dm_target {
                    let dest = *dest;
                    let name = name.clone();
                    self.push_system(format!("Sending file: {} to {}", filename, name));
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_file(dest, &path_str).await });
                } else if !self.peers.is_empty() {
                    // No DM target: show a message, user should pick a DM target first
                    self.push_system(format!("Select a peer first (click sidebar), then send file: {}", filename));
                } else {
                    self.push_system("No peers to send file to".into());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// egui App implementation
// ---------------------------------------------------------------------------

impl eframe::App for MeshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain incoming mesh events
        while let Ok(ev) = self.bridge_rx.try_recv() {
            self.handle_mesh_event(ev);
        }

        // Check file picker results
        self.check_file_pick_result();

        // Expire typing indicators (5 second timeout)
        self.typing_peers.retain(|(_, _, t)| t.elapsed().as_secs() < 5);

        // Expire disappearing messages
        self.disappearing_msgs.retain(|(_, _, received, ttl)| {
            received.elapsed().as_secs() < *ttl as u64
        });

        if self.should_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Keyboard shortcuts (global)
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F1) { self.active_tab = Tab::Chat; }
            if i.key_pressed(egui::Key::F2) { self.active_tab = Tab::Peers; }
            if i.key_pressed(egui::Key::F3) { self.active_tab = Tab::Files; }
            if i.key_pressed(egui::Key::F4) {
                self.active_tab = Tab::Settings;
                let h = self.handle.clone();
                self.spawn_cmd(async move { h.get_stats().await });
            }
            if i.key_pressed(egui::Key::Escape) {
                if self.dm_target.is_some() {
                    self.dm_target = None;
                }
            }
        });

        // Apply dark theme
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG_DARK;
        visuals.window_fill = BG_DARK;
        visuals.extreme_bg_color = BG_INPUT;
        visuals.widgets.noninteractive.bg_fill = BG_DARK;
        visuals.widgets.inactive.bg_fill = BG_INPUT;
        visuals.widgets.inactive.weak_bg_fill = BG_INPUT;
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(55, 57, 63);
        visuals.widgets.active.bg_fill = Color32::from_rgb(65, 67, 75);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        visuals.selection.bg_fill = ACCENT_BLUE.linear_multiply(0.3);
        visuals.window_corner_radius = CornerRadius::same(8);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(4);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(4);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(4);
        visuals.widgets.active.corner_radius = CornerRadius::same(4);
        ctx.set_visuals(visuals);

        // Header panel
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("MassKritical").font(FontId::proportional(16.0)).color(ACCENT_CYAN).strong());
                ui.separator();
                ui.label(RichText::new(format!("{} ({})", self.display_name, self.node_id_short)).color(TEXT_PRIMARY));
                ui.separator();
                ui.label(RichText::new(format!("Peers: {}", self.peers.len())).color(ACCENT_GREEN));
                ui.separator();
                ui.label(RichText::new(format!("Port: {}", self.port)).color(TEXT_MUTED));
                ui.separator();
                let gw = self.gateway_name.as_deref().unwrap_or("none");
                ui.label(RichText::new(format!("GW: {}", gw)).color(ACCENT_YELLOW));

                // Connectivity indicator
                if !self.stats.active_interface.is_empty() {
                    ui.separator();
                    if let Some(iface) = self.stats.interfaces.iter().find(|i| i.active) {
                        let label = format!("{} ({})", iface.if_type.display_name(), iface.ip);
                        ui.label(RichText::new(label).color(ACCENT_GREEN));
                    }
                }
            });

            // Call banner
            if let Some((_, ref name)) = self.in_call {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("In call with {}", name)).color(ACCENT_MAGENTA).strong());
                    if ui.button(RichText::new("End Call").color(ACCENT_RED)).clicked() {
                        // Can't call end_call here due to borrow, handled below
                    }
                });
            }

            ui.add_space(2.0);
        });

        // Tab bar
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // I'm OK button in tab bar area
                let ok_btn = egui::Button::new(
                    RichText::new("I'm OK").color(Color32::WHITE).strong()
                ).fill(ACCENT_GREEN);
                if ui.add(ok_btn).clicked() {
                    let payload = CheckInPayload {
                        status: "ok".to_string(),
                        location: None,
                        message: String::new(),
                    };
                    self.push_system("CHECK-IN: ok".into());
                    let h = self.handle.clone();
                    self.spawn_cmd(async move { h.send_check_in(payload).await });
                }
                ui.separator();

                for (tab, label) in [
                    (Tab::Chat, "Chat"),
                    (Tab::Peers, "Peers"),
                    (Tab::Files, "Files"),
                    (Tab::Emergency, "Emergency"),
                    (Tab::Settings, "Settings"),
                ] {
                    let selected = self.active_tab == tab;
                    let text = if selected {
                        RichText::new(label).color(BG_DARK).strong()
                    } else {
                        RichText::new(label).color(TEXT_MUTED)
                    };
                    let btn = egui::Button::new(text)
                        .fill(if selected { ACCENT_CYAN } else { Color32::TRANSPARENT })
                        .corner_radius(CornerRadius::same(4));
                    if ui.add(btn).clicked() {
                        self.active_tab = tab;
                        if tab == Tab::Settings {
                            let h = self.handle.clone();
                            self.spawn_cmd(async move { h.get_stats().await });
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("F1-F4: tabs  Esc: exit DM").font(FontId::proportional(11.0)).color(TEXT_MUTED));
                });
            });
        });

        match self.active_tab {
            Tab::Chat => self.draw_chat_tab(ctx),
            Tab::Peers => self.draw_peers_tab(ctx),
            Tab::Files => self.draw_files_tab(ctx),
            Tab::Emergency => self.draw_emergency_tab(ctx),
            Tab::Settings => self.draw_settings_tab(ctx),
        }

        // Nuke confirmation dialog
        if self.show_nuke_confirm {
            self.draw_nuke_dialog(ctx);
        }

        // File offer dialog
        if self.pending_file_offer.is_some() {
            self.draw_file_offer_dialog(ctx);
        }

        // Incoming call dialog
        if self.incoming_call.is_some() {
            self.draw_incoming_call_dialog(ctx);
        }
    }
}

// ---------------------------------------------------------------------------
// Tab drawing
// ---------------------------------------------------------------------------

impl MeshApp {
    fn draw_chat_tab(&mut self, ctx: &egui::Context) {
        // Bottom panel: input bar
        egui::TopBottomPanel::bottom("input_bar").show(ctx, |ui| {
            ui.add_space(4.0);

            // Call banner in chat
            if let Some((_, name)) = self.in_call.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("In call with {}", name)).color(ACCENT_MAGENTA).strong());
                    if ui.button(RichText::new("End Call").color(ACCENT_RED).strong()).clicked() {
                        self.end_call();
                    }
                });
                ui.separator();
            }

            // Typing indicator
            if !self.typing_peers.is_empty() {
                let names: Vec<String> = self.typing_peers.iter().map(|(_, n, _)| n.clone()).collect();
                let typing_text = if names.len() == 1 {
                    format!("{} is typing...", names[0])
                } else {
                    format!("{} are typing...", names.join(", "))
                };
                ui.label(RichText::new(typing_text).color(TEXT_MUTED).italics().font(FontId::proportional(11.0)));
            }

            if let Some(dm_name) = self.dm_target.as_ref().map(|(_, n)| n.clone()) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("DMing: {}", dm_name)).color(ACCENT_MAGENTA).strong());
                    if ui.small_button(RichText::new("x").color(ACCENT_RED).strong()).clicked() {
                        self.dm_target = None;
                    }
                    ui.label(RichText::new("(Esc to exit)").font(FontId::proportional(11.0)).color(TEXT_MUTED));
                });
            }
            ui.horizontal(|ui| {
                let prompt = if self.dm_target.is_some() { "DM >" } else { ">" };
                ui.label(RichText::new(prompt).color(ACCENT_GREEN).strong());

                let response = ui.add_sized(
                    Vec2::new(ui.available_width() - 140.0, 24.0),
                    egui::TextEdit::singleline(&mut self.input)
                        .font(FontId::proportional(14.0))
                        .text_color(TEXT_PRIMARY)
                        .desired_width(f32::INFINITY),
                );

                // Auto-focus the input field
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.handle_input();
                    response.request_focus();
                }

                // Auto-focus input when nothing else has focus
                if !ctx.memory(|m| m.has_focus(response.id)) && !response.has_focus() {
                    response.request_focus();
                }

                // Mic button (voice note)
                let mic_label = if self.recording {
                    let elapsed = self.record_start.map(|s| s.elapsed().as_secs()).unwrap_or(0);
                    RichText::new(format!("REC {}s", elapsed)).color(ACCENT_RED).strong()
                } else {
                    RichText::new("Mic").color(ACCENT_YELLOW)
                };
                let mic_btn = egui::Button::new(mic_label)
                    .fill(if self.recording { ACCENT_RED.linear_multiply(0.2) } else { Color32::TRANSPARENT });
                if ui.add(mic_btn).clicked() {
                    if self.recording {
                        self.stop_recording_and_send();
                    } else {
                        self.start_recording();
                    }
                }

                // Request repaint while recording to update timer
                if self.recording {
                    ctx.request_repaint();
                }

                // File attach button
                if ui.button(RichText::new("+").color(ACCENT_BLUE)).clicked() {
                    self.open_file_picker();
                }

                if ui.button(RichText::new("Send").color(ACCENT_CYAN)).clicked() {
                    self.handle_input();
                    response.request_focus();
                }
            });
            ui.add_space(2.0);
        });

        // Left panel: peer sidebar with groups
        egui::SidePanel::left("peer_sidebar")
            .default_width(180.0)
            .min_width(120.0)
            .frame(egui::Frame::new().fill(BG_SIDEBAR).inner_margin(8.0))
            .show(ctx, |ui| {
                // Groups section
                if !self.joined_groups.is_empty() {
                    ui.label(RichText::new("Groups").color(ACCENT_BLUE).strong());
                    let groups = self.joined_groups.clone();
                    for g in &groups {
                        let selected = self.active_group.as_deref() == Some(g);
                        let color = if selected { ACCENT_CYAN } else { ACCENT_BLUE };
                        let btn = ui.add(
                            egui::Button::new(RichText::new(format!("# {}", g)).color(color).font(FontId::proportional(12.0)))
                                .fill(if selected { ACCENT_BLUE.linear_multiply(0.15) } else { Color32::TRANSPARENT })
                                .frame(false),
                        );
                        if btn.clicked() {
                            self.active_group = Some(g.clone());
                            self.dm_target = None;
                        }
                    }
                    if ui.small_button(RichText::new("Clear group").color(TEXT_MUTED)).clicked() {
                        self.active_group = None;
                    }
                    ui.separator();
                }

                ui.label(RichText::new("Peers").color(ACCENT_YELLOW).strong());
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let peer_data: Vec<([u8; 32], String, bool)> = self.peers.iter()
                        .map(|p| (p.node_id, p.display_name.clone(), p.is_gateway))
                        .collect();

                    for (node_id, display_name, is_gateway) in &peer_data {
                        let short = hex::encode(&node_id[..4]);
                        let gw_tag = if *is_gateway { " [GW]" } else { "" };
                        let color = if *is_gateway { ACCENT_YELLOW } else { ACCENT_GREEN };

                        ui.horizontal(|ui| {
                            let label = format!("{} ({}){}", display_name, short, gw_tag);
                            let btn = ui.add(
                                egui::Button::new(RichText::new(&label).color(color).font(FontId::proportional(12.0)))
                                    .fill(Color32::TRANSPARENT)
                                    .frame(false),
                            );
                            if btn.clicked() {
                                self.dm_target = Some((*node_id, display_name.clone()));
                                self.active_group = None;
                            }
                            if btn.hovered() {
                                ui.painter().rect_stroke(
                                    btn.rect.expand(2.0),
                                    CornerRadius::same(4),
                                    Stroke::new(1.0, ACCENT_CYAN.linear_multiply(0.4)),
                                    StrokeKind::Outside,
                                );
                            }
                        });
                    }

                    if peer_data.is_empty() {
                        ui.label(RichText::new("No peers").color(TEXT_MUTED).italics());
                    }
                });
            });

        // Central panel: messages
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(8.0))
            .show(ctx, |ui| {
                let title = if let Some((_, ref name)) = self.dm_target {
                    format!("Messages - DM: {}", name)
                } else {
                    "Messages".to_string()
                };
                ui.label(RichText::new(title).color(ACCENT_YELLOW).strong());
                ui.separator();

                // Collect voice note play actions
                let mut play_voice: Option<(Vec<u8>, u32)> = None;

                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for msg in &self.messages {
                            match &msg.kind {
                                ChatMessageKind::System => {
                                    ui.label(
                                        RichText::new(format!("  * {}", msg.content))
                                            .color(TEXT_MUTED)
                                            .italics()
                                            .font(FontId::proportional(13.0)),
                                    );
                                }
                                ChatMessageKind::Dm => {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!("[DM {}]", msg.sender))
                                                .color(ACCENT_MAGENTA)
                                                .strong()
                                                .font(FontId::proportional(13.0)),
                                        );
                                        ui.label(
                                            RichText::new(&msg.content)
                                                .color(TEXT_PRIMARY)
                                                .font(FontId::proportional(13.0)),
                                        );
                                    });
                                }
                                ChatMessageKind::Public => {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!("[{}]", msg.sender))
                                                .color(ACCENT_BLUE)
                                                .strong()
                                                .font(FontId::proportional(13.0)),
                                        );
                                        ui.label(
                                            RichText::new(&msg.content)
                                                .color(TEXT_PRIMARY)
                                                .font(FontId::proportional(13.0)),
                                        );
                                    });
                                }
                                ChatMessageKind::VoiceNote { audio_data, duration_ms } => {
                                    ui.horizontal(|ui| {
                                        let secs = *duration_ms as f64 / 1000.0;
                                        ui.label(
                                            RichText::new(format!("[Voice] {} ({:.1}s)", msg.sender, secs))
                                                .color(ACCENT_YELLOW)
                                                .strong()
                                                .font(FontId::proportional(13.0)),
                                        );
                                        let play_btn = egui::Button::new(
                                            RichText::new("Play").color(ACCENT_CYAN).font(FontId::proportional(12.0))
                                        ).fill(Color32::TRANSPARENT);
                                        if ui.add(play_btn).clicked() {
                                            play_voice = Some((audio_data.clone(), *duration_ms));
                                        }
                                    });
                                }
                            }
                        }
                    });

                // Play voice note if requested
                if let Some((data, dur)) = play_voice {
                    self.play_voice_note(&data, dur);
                }
            });
    }

    fn draw_peers_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(12.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("Peers (click name to DM, Call to call)").color(ACCENT_YELLOW).strong());
                ui.add_space(8.0);

                let available_height = ui.available_height();
                let text_height = 20.0;

                // Collect peer data before building the table
                let peer_rows: Vec<(usize, String, String, bool, String)> = self.peers.iter().enumerate()
                    .map(|(i, p)| {
                        let short = hex::encode(&p.node_id[..4]);
                        (i, p.display_name.clone(), short, p.is_gateway, p.bio.clone())
                    })
                    .collect();

                let mut clicked_peer: Option<([u8; 32], String)> = None;
                let mut call_peer: Option<([u8; 32], String)> = None;

                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(available_height)
                    .column(Column::initial(140.0).at_least(80.0))
                    .column(Column::initial(80.0).at_least(60.0))
                    .column(Column::initial(40.0).at_least(30.0))
                    .column(Column::initial(60.0).at_least(40.0))
                    .column(Column::remainder().at_least(80.0))
                    .header(text_height, |mut header| {
                        header.col(|ui| { ui.label(RichText::new("Name").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("ID").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("GW").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("Call").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("Bio").color(ACCENT_YELLOW).strong()); });
                    })
                    .body(|mut body| {
                        for (i, name, short, is_gw, bio) in &peer_rows {
                            body.row(text_height, |mut row| {
                                row.col(|ui| {
                                    let color = if *is_gw { ACCENT_YELLOW } else { ACCENT_GREEN };
                                    if ui.add(egui::Label::new(RichText::new(name).color(color)).sense(egui::Sense::click())).clicked() {
                                        clicked_peer = Some((self.peers[*i].node_id, name.clone()));
                                    }
                                });
                                row.col(|ui| { ui.label(RichText::new(short).color(TEXT_MUTED)); });
                                row.col(|ui| {
                                    let gw_text = if *is_gw { "Yes" } else { "No" };
                                    ui.label(RichText::new(gw_text).color(TEXT_MUTED));
                                });
                                row.col(|ui| {
                                    let call_btn = egui::Button::new(
                                        RichText::new("Call").color(ACCENT_GREEN).font(FontId::proportional(11.0))
                                    ).fill(Color32::TRANSPARENT);
                                    if ui.add(call_btn).clicked() {
                                        call_peer = Some((self.peers[*i].node_id, name.clone()));
                                    }
                                });
                                row.col(|ui| { ui.label(RichText::new(bio).color(TEXT_MUTED)); });
                            });
                        }
                    });

                if let Some((node_id, name)) = clicked_peer {
                    self.dm_target = Some((node_id, name));
                    self.active_tab = Tab::Chat;
                }

                if let Some((node_id, name)) = call_peer {
                    self.start_call(node_id, name);
                }
            });
    }

    fn draw_files_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(12.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("File Transfers").color(ACCENT_YELLOW).strong());
                ui.add_space(8.0);

                let available_height = ui.available_height();
                let text_height = 22.0;

                let file_rows: Vec<(String, String, String, u8, bool)> = self.files.iter()
                    .map(|f| {
                        let dir = if f.incoming { "IN" } else { "OUT" };
                        (dir.to_string(), f.filename.clone(), format_size(f.size), f.progress, f.done)
                    })
                    .collect();

                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(available_height)
                    .column(Column::initial(50.0).at_least(30.0))
                    .column(Column::initial(220.0).at_least(100.0))
                    .column(Column::initial(90.0).at_least(60.0))
                    .column(Column::remainder().at_least(100.0))
                    .header(text_height, |mut header| {
                        header.col(|ui| { ui.label(RichText::new("Dir").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("Filename").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("Size").color(ACCENT_YELLOW).strong()); });
                        header.col(|ui| { ui.label(RichText::new("Status").color(ACCENT_YELLOW).strong()); });
                    })
                    .body(|mut body| {
                        for (dir, filename, size, progress, done) in &file_rows {
                            body.row(text_height, |mut row| {
                                row.col(|ui| {
                                    let color = if dir == "IN" { ACCENT_GREEN } else { ACCENT_BLUE };
                                    ui.label(RichText::new(dir).color(color));
                                });
                                row.col(|ui| { ui.label(RichText::new(filename).color(TEXT_PRIMARY)); });
                                row.col(|ui| { ui.label(RichText::new(size).color(TEXT_MUTED)); });
                                row.col(|ui| {
                                    if *done {
                                        ui.label(RichText::new("Complete").color(ACCENT_GREEN));
                                    } else {
                                        ui.add(
                                            egui::ProgressBar::new(*progress as f32 / 100.0)
                                                .text(format!("{}%", progress))
                                                .fill(ACCENT_CYAN),
                                        );
                                    }
                                });
                            });
                        }
                    });

                if file_rows.is_empty() {
                    ui.add_space(20.0);
                    ui.label(RichText::new("No file transfers").color(TEXT_MUTED).italics());
                }
            });
    }

    fn draw_emergency_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(12.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // --- Triage Section ---
                    ui.label(RichText::new("Triage Tags").color(ACCENT_RED).strong().font(FontId::proportional(16.0)));
                    ui.add_space(4.0);

                    // Summary
                    let (mut r, mut y, mut g, mut b) = (0, 0, 0, 0);
                    for (_, t) in &self.triage_log {
                        match t.level {
                            TriageLevel::Red => r += 1,
                            TriageLevel::Yellow => y += 1,
                            TriageLevel::Green => g += 1,
                            TriageLevel::Black => b += 1,
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("R:{}", r)).color(ACCENT_RED).strong());
                        ui.label(RichText::new(format!("Y:{}", y)).color(ACCENT_YELLOW).strong());
                        ui.label(RichText::new(format!("G:{}", g)).color(ACCENT_GREEN).strong());
                        ui.label(RichText::new(format!("B:{}", b)).color(TEXT_MUTED).strong());
                    });
                    ui.add_space(4.0);

                    // Triage input
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Level:").color(ACCENT_YELLOW));
                        for (lvl, color) in [("red", ACCENT_RED), ("yellow", ACCENT_YELLOW), ("green", ACCENT_GREEN), ("black", TEXT_MUTED)] {
                            let selected = self.triage_level == lvl;
                            let btn = egui::Button::new(RichText::new(lvl.to_uppercase()).color(if selected { BG_DARK } else { color }).strong())
                                .fill(if selected { color } else { Color32::TRANSPARENT });
                            if ui.add(btn).clicked() {
                                self.triage_level = lvl.into();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Victim ID:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.triage_victim_id).desired_width(100.0));
                        ui.label(RichText::new("Notes:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.triage_notes).desired_width(200.0));
                        let send_btn = egui::Button::new(RichText::new("Send Triage").color(Color32::WHITE).strong()).fill(ACCENT_RED);
                        if ui.add(send_btn).clicked() && !self.triage_victim_id.is_empty() {
                            if let Some(level) = TriageLevel::from_str(&self.triage_level) {
                                let payload = TriagePayload {
                                    level, victim_id: self.triage_victim_id.clone(),
                                    notes: self.triage_notes.clone(), location: None,
                                };
                                self.triage_log.push(("You".into(), payload.clone()));
                                let h = self.handle.clone();
                                self.spawn_cmd(async move { h.send_triage(payload).await });
                                self.triage_victim_id.clear();
                                self.triage_notes.clear();
                            }
                        }
                    });

                    // Triage log
                    ui.add_space(4.0);
                    for (sender, t) in self.triage_log.iter().rev().take(20) {
                        let color = match t.level {
                            TriageLevel::Red => ACCENT_RED,
                            TriageLevel::Yellow => ACCENT_YELLOW,
                            TriageLevel::Green => ACCENT_GREEN,
                            TriageLevel::Black => TEXT_MUTED,
                        };
                        ui.label(RichText::new(format!(
                            "[{}] {} - victim:{} - {}", t.level.label(), sender, t.victim_id, t.notes
                        )).color(color).font(FontId::proportional(12.0)));
                    }

                    ui.add_space(12.0);
                    ui.separator();

                    // --- Resource Requests ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Resource Requests").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Category:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.resource_category).desired_width(100.0));
                        ui.label(RichText::new("Urgency:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.resource_urgency).desired_width(30.0));
                        ui.label(RichText::new("Desc:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.resource_description).desired_width(200.0));
                        let send_btn = egui::Button::new(RichText::new("Request").color(Color32::WHITE).strong()).fill(ACCENT_BLUE);
                        if ui.add(send_btn).clicked() && !self.resource_description.is_empty() {
                            let urgency: u8 = self.resource_urgency.parse().unwrap_or(3);
                            let payload = ResourceRequestPayload {
                                category: self.resource_category.clone(),
                                description: self.resource_description.clone(),
                                urgency, location: None, quantity: 1,
                            };
                            self.resource_log.push(("You".into(), payload.clone()));
                            let h = self.handle.clone();
                            self.spawn_cmd(async move { h.send_resource_request(payload).await });
                            self.resource_description.clear();
                        }
                    });

                    for (sender, r) in self.resource_log.iter().rev().take(20) {
                        let urgency_color = match r.urgency {
                            5 => ACCENT_RED, 4 => ACCENT_MAGENTA, 3 => ACCENT_YELLOW,
                            _ => ACCENT_GREEN,
                        };
                        ui.label(RichText::new(format!(
                            "[{} U:{}] {} - {}", r.category, r.urgency, sender, r.description
                        )).color(urgency_color).font(FontId::proportional(12.0)));
                    }

                    ui.add_space(12.0);
                    ui.separator();

                    // --- Safety Roster ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Safety Roster").color(ACCENT_GREEN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(4.0);

                    if self.safety_roster.is_empty() {
                        ui.label(RichText::new("No check-ins received yet").color(TEXT_MUTED).italics());
                    }
                    for (_, name, status, time) in &self.safety_roster {
                        let age_secs = time.elapsed().as_secs();
                        let color = match status.as_str() {
                            "ok" if age_secs < 3600 => ACCENT_GREEN,
                            "ok" => ACCENT_YELLOW,
                            "need_help" => ACCENT_RED,
                            "evacuating" => ACCENT_MAGENTA,
                            _ => TEXT_MUTED,
                        };
                        let age_str = if age_secs < 60 { format!("{}s ago", age_secs) }
                            else if age_secs < 3600 { format!("{}m ago", age_secs / 60) }
                            else { format!("{}h ago", age_secs / 3600) };
                        ui.label(RichText::new(format!(
                            "{}: {} ({})", name, status, age_str
                        )).color(color).font(FontId::proportional(12.0)));
                    }
                });
            });
    }

    fn draw_settings_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(12.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // --- Profile Section ---
                    ui.label(RichText::new("Profile").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    egui::Grid::new("profile_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Display Name:").color(ACCENT_YELLOW));
                            ui.add(egui::TextEdit::singleline(&mut self.settings_name)
                                .desired_width(200.0)
                                .font(FontId::proportional(14.0)));
                            ui.end_row();

                            ui.label(RichText::new("Bio:").color(ACCENT_YELLOW));
                            ui.add(egui::TextEdit::singleline(&mut self.bio)
                                .desired_width(200.0)
                                .hint_text("About you...")
                                .font(FontId::proportional(14.0)));
                            ui.end_row();
                        });

                    ui.add_space(4.0);
                    let update_btn = egui::Button::new(
                        RichText::new("Update Profile").color(Color32::WHITE).strong()
                    ).fill(ACCENT_BLUE);
                    if ui.add(update_btn).clicked() {
                        self.display_name = self.settings_name.clone();
                        let name = self.settings_name.clone();
                        let bio = self.bio.clone();
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.update_profile(&name, &bio).await });
                        self.push_system(format!("Profile updated: {}", self.display_name));
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Node Info Section ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Node Info").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    egui::Grid::new("nodeinfo_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("Node ID:").color(ACCENT_YELLOW));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&self.node_id_hex).color(TEXT_MUTED).font(FontId::monospace(11.0)));
                                if ui.small_button("Copy").clicked() {
                                    #[allow(deprecated)]
                                    ui.output_mut(|o| o.copied_text = self.node_id_hex.clone());
                                }
                            });
                            ui.end_row();

                            ui.label(RichText::new("Encryption:").color(ACCENT_YELLOW));
                            ui.label(RichText::new("X25519 + ChaCha20-Poly1305").color(TEXT_PRIMARY));
                            ui.end_row();

                            ui.label(RichText::new("Port:").color(ACCENT_YELLOW));
                            ui.label(RichText::new(format!("{}", self.port)).color(TEXT_PRIMARY));
                            ui.end_row();
                        });

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Connectivity Section ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Connectivity").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    if self.stats.interfaces.is_empty() {
                        ui.label(RichText::new("No interface data (request stats with F4)").color(TEXT_MUTED).italics());
                    } else {
                        egui::Grid::new("iface_grid")
                            .num_columns(4)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Interface").color(ACCENT_YELLOW).strong());
                                ui.label(RichText::new("Type").color(ACCENT_YELLOW).strong());
                                ui.label(RichText::new("IP").color(ACCENT_YELLOW).strong());
                                ui.label(RichText::new("Status").color(ACCENT_YELLOW).strong());
                                ui.end_row();

                                for iface in &self.stats.interfaces {
                                    let color = if iface.active { ACCENT_GREEN } else { TEXT_MUTED };
                                    ui.label(RichText::new(&iface.name).color(color));
                                    ui.label(RichText::new(iface.if_type.display_name()).color(color));
                                    ui.label(RichText::new(&iface.ip).color(color));
                                    let status = if iface.active { "Active" } else { "Available" };
                                    ui.label(RichText::new(status).color(color));
                                    ui.end_row();
                                }
                            });
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Mesh Statistics Section ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Mesh Statistics").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    egui::Grid::new("stats_grid")
                        .num_columns(2)
                        .spacing([20.0, 8.0])
                        .show(ui, |ui| {
                            let stat = |ui: &mut egui::Ui, label: &str, value: String| {
                                ui.label(RichText::new(label).color(ACCENT_YELLOW));
                                ui.label(RichText::new(value).color(TEXT_PRIMARY));
                                ui.end_row();
                            };

                            stat(ui, "Connected Peers:", format!("{}", self.peers.len()));
                            stat(ui, "Messages Relayed:", format!("{}", self.stats.messages_relayed));
                            stat(ui, "Messages Received:", format!("{}", self.stats.messages_received));
                            stat(ui, "Unique Nodes Seen:", format!("{}", self.stats.unique_nodes_seen));
                            stat(ui, "Average Hops:", format!("{:.1}", self.stats.avg_hops));

                            ui.label(RichText::new("").color(TEXT_MUTED));
                            ui.label(RichText::new("").color(TEXT_MUTED));
                            ui.end_row();

                            stat(ui, "Gateway:", self.gateway_name.as_deref().unwrap_or("None").to_string());
                            stat(ui, "Active Files:", format!("{}", self.files.iter().filter(|f| !f.done).count()));
                        });

                    ui.add_space(8.0);
                    if ui.button(RichText::new("Refresh Stats").color(ACCENT_CYAN)).clicked() {
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.get_stats().await });
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Groups Section ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Groups").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Join group:").color(ACCENT_YELLOW));
                        ui.add(egui::TextEdit::singleline(&mut self.group_join_input)
                            .desired_width(150.0)
                            .hint_text("group name"));
                        if ui.button(RichText::new("Join").color(ACCENT_GREEN)).clicked() && !self.group_join_input.is_empty() {
                            let name = self.group_join_input.clone();
                            self.joined_groups.push(name.clone());
                            self.group_join_input.clear();
                            let h = self.handle.clone();
                            self.spawn_cmd(async move { h.join_group(&name).await });
                        }
                    });

                    let groups_snapshot = self.joined_groups.clone();
                    for g in &groups_snapshot {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("# {}", g)).color(ACCENT_BLUE));
                            if ui.small_button(RichText::new("Leave").color(ACCENT_RED)).clicked() {
                                self.joined_groups.retain(|x| x != g);
                                if self.active_group.as_deref() == Some(g) {
                                    self.active_group = None;
                                }
                                let name = g.clone();
                                let h = self.handle.clone();
                                self.spawn_cmd(async move { h.leave_group(&name).await });
                            }
                        });
                    }

                    if groups_snapshot.is_empty() {
                        ui.label(RichText::new("Not in any groups").color(TEXT_MUTED).italics());
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Key Verification ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Key Verification").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    let peer_data_for_verify: Vec<([u8; 32], String)> = self.peers.iter()
                        .map(|p| (p.node_id, p.display_name.clone()))
                        .collect();
                    for (peer_id, peer_name) in &peer_data_for_verify {
                        let safety = NodeIdentity::safety_number(&self.our_node_id, peer_id);
                        ui.collapsing(RichText::new(format!("Verify: {}", peer_name)).color(ACCENT_YELLOW), |ui| {
                            ui.label(RichText::new(&safety).color(TEXT_PRIMARY).font(FontId::monospace(12.0)));
                        });
                    }

                    if peer_data_for_verify.is_empty() {
                        ui.label(RichText::new("No peers to verify").color(TEXT_MUTED).italics());
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Notifications ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Notifications").color(ACCENT_CYAN).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.notifications_muted, RichText::new("Mute notifications").color(TEXT_PRIMARY));

                    ui.add_space(16.0);
                    ui.separator();

                    // --- Danger Zone ---
                    ui.add_space(8.0);
                    ui.label(RichText::new("Danger Zone").color(ACCENT_RED).strong().font(FontId::proportional(16.0)));
                    ui.add_space(8.0);

                    let nuke_btn = egui::Button::new(
                        RichText::new("NUKE - Destroy All Data").color(Color32::WHITE).strong(),
                    ).fill(ACCENT_RED);
                    if ui.add(nuke_btn).clicked() {
                        self.show_nuke_confirm = true;
                    }
                });
            });
    }

    fn draw_nuke_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Destroy Identity?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui::Frame::window(&ctx.style()).fill(Color32::from_rgb(40, 20, 20)).inner_margin(20.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("This will permanently destroy your node identity.")
                        .color(ACCENT_RED)
                        .strong(),
                );
                ui.label(
                    RichText::new("Your keypair will be deleted and cannot be recovered.")
                        .color(TEXT_PRIMARY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Cancel").color(TEXT_PRIMARY)).clicked() {
                        self.show_nuke_confirm = false;
                    }
                    ui.add_space(20.0);
                    let nuke_btn = egui::Button::new(
                        RichText::new("NUKE").color(Color32::WHITE).strong(),
                    )
                    .fill(ACCENT_RED);
                    if ui.add(nuke_btn).clicked() {
                        self.show_nuke_confirm = false;
                        self.push_system("Destroying identity...".into());
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.nuke().await });
                    }
                });
            });
    }

    fn draw_file_offer_dialog(&mut self, ctx: &egui::Context) {
        let (file_id, sender, filename, size) = self.pending_file_offer.clone().unwrap();
        let size_str = format_size(size);

        egui::Window::new("Incoming File")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui::Frame::window(&ctx.style()).fill(Color32::from_rgb(25, 35, 45)).inner_margin(20.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("{} wants to send you a file:", sender))
                        .color(ACCENT_CYAN)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("File:").color(ACCENT_YELLOW));
                    ui.label(RichText::new(&filename).color(TEXT_PRIMARY).strong());
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Size:").color(ACCENT_YELLOW));
                    ui.label(RichText::new(&size_str).color(TEXT_PRIMARY));
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let accept_btn = egui::Button::new(
                        RichText::new("Accept").color(Color32::WHITE).strong(),
                    )
                    .fill(ACCENT_GREEN);
                    if ui.add(accept_btn).clicked() {
                        self.pending_file_offer = None;
                        self.push_system(format!("Accepting file: {}", filename));
                        let h = self.handle.clone();
                        self.spawn_cmd(async move { h.accept_file(file_id).await });
                    }
                    ui.add_space(20.0);
                    let decline_btn = egui::Button::new(
                        RichText::new("Decline").color(TEXT_PRIMARY),
                    );
                    if ui.add(decline_btn).clicked() {
                        self.pending_file_offer = None;
                        self.push_system(format!("Declined file: {}", filename));
                    }
                });
            });
    }

    fn draw_incoming_call_dialog(&mut self, ctx: &egui::Context) {
        let (_, peer_name) = self.incoming_call.clone().unwrap();

        egui::Window::new("Incoming Call")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui::Frame::window(&ctx.style()).fill(Color32::from_rgb(25, 35, 45)).inner_margin(20.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("Incoming call from {}", peer_name))
                        .color(ACCENT_CYAN)
                        .strong()
                        .font(FontId::proportional(16.0)),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let accept_btn = egui::Button::new(
                        RichText::new("Accept").color(Color32::WHITE).strong(),
                    ).fill(ACCENT_GREEN);
                    if ui.add(accept_btn).clicked() {
                        self.accept_call();
                    }
                    ui.add_space(20.0);
                    let decline_btn = egui::Button::new(
                        RichText::new("Decline").color(Color32::WHITE).strong(),
                    ).fill(ACCENT_RED);
                    if ui.add(decline_btn).clicked() {
                        self.decline_call();
                    }
                });
            });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn whoami() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into())
}

// ---------------------------------------------------------------------------
// Audio resampling
// ---------------------------------------------------------------------------

/// Resample 16kHz mono i16 audio to the device's native rate and channel count.
/// Returns f32 samples interleaved for the target channel count.
fn resample_audio(mono_16k: &[i16], src_rate: u32, dst_rate: u32, dst_channels: usize) -> Vec<f32> {
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((mono_16k.len() as f64) / ratio).ceil() as usize;
    let mut result = Vec::with_capacity(out_len * dst_channels);

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        let s0 = mono_16k.get(idx).copied().unwrap_or(0) as f32 / 32768.0;
        let s1 = mono_16k.get(idx + 1).copied().unwrap_or(mono_16k.get(idx).copied().unwrap_or(0)) as f32 / 32768.0;
        let sample = s0 + (s1 - s0) * frac as f32;

        // Duplicate to all channels
        for _ in 0..dst_channels {
            result.push(sample);
        }
    }
    result
}

// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    // Parse args
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| {
        let hostname = whoami();
        format!("mesh-{}", &hostname[..hostname.len().min(8)])
    });
    let port: u16 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7332);

    // Build tokio runtime manually (eframe owns the main thread)
    let rt = Arc::new(tokio::runtime::Runtime::new()?);

    // Start mesh node on the tokio runtime
    let (identity, handle, mut event_rx) = rt.block_on(async {
        let config = NodeConfig {
            display_name: name.clone(),
            listen_port: port,
            key_path: std::path::PathBuf::from(format!("mesh_identity_{}.key", port)),
            data_dir: None,
        };
        start_mesh_node(config).await
    })?;

    let node_id_hex = identity.node_id_hex();
    let node_id_short = identity.node_id_short();

    // Bridge channel: tokio events -> GUI thread (std::sync::mpsc)
    let (bridge_tx, bridge_rx) = std_mpsc::channel::<NodeEvent>();

    // Spawn bridge task that forwards events and requests repaints
    let rt_clone = rt.clone();
    let egui_ctx: Arc<StdMutex<Option<egui::Context>>> =
        Arc::new(StdMutex::new(None));
    let egui_ctx_clone = egui_ctx.clone();

    rt_clone.spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if bridge_tx.send(ev).is_err() {
                break;
            }
            if let Ok(guard) = egui_ctx_clone.lock() {
                if let Some(ref ctx) = *guard {
                    ctx.request_repaint();
                }
            }
        }
    });

    let our_node_id = identity.node_id;

    let app = MeshApp {
        display_name: name.clone(),
        node_id_hex,
        node_id_short,
        port,
        messages: Vec::new(),
        peers: Vec::new(),
        files: Vec::new(),
        stats: MeshStats::default(),
        gateway_name: None,
        input: String::new(),
        active_tab: Tab::Chat,
        dm_target: None,
        show_nuke_confirm: false,
        pending_file_offer: None,
        should_quit: false,
        settings_name: name.clone(),
        bio: String::new(),
        recording: false,
        record_start: None,
        audio_buffer: Arc::new(StdMutex::new(Vec::new())),
        audio_input_stream: None,
        playback_active: false,
        audio_output_stream: None,
        in_call: None,
        incoming_call: None,
        call_input_stream: None,
        call_output_stream: None,
        call_playback_buffer: Arc::new(StdMutex::new(VecDeque::new())),
        file_pick_rx: None,
        joined_groups: Vec::new(),
        active_group: None,
        group_join_input: String::new(),
        typing_peers: Vec::new(),
        _typing_sent: false,
        _last_typed: None,
        triage_log: Vec::new(),
        resource_log: Vec::new(),
        safety_roster: Vec::new(),
        disappearing_msgs: Vec::new(),
        triage_victim_id: String::new(),
        triage_notes: String::new(),
        triage_level: "red".into(),
        resource_category: "medical".into(),
        resource_description: String::new(),
        resource_urgency: "3".into(),
        notifications_muted: false,
        our_node_id,
        handle,
        rt,
        bridge_rx,
    };

    // Load window icon from embedded ICO
    let icon = {
        let icon_bytes = include_bytes!("../../design/logo1.png");
        match image::load_from_memory_with_format(icon_bytes, image::ImageFormat::Png) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                Some(egui::IconData { rgba: rgba.into_raw(), width: w, height: h })
            }
            Err(_) => None,
        }
    };

    let mut vp = egui::ViewportBuilder::default()
        .with_title(format!("MassKritical - {}", name))
        .with_inner_size([900.0, 640.0])
        .with_min_inner_size([600.0, 400.0]);
    if let Some(icon_data) = icon {
        vp = vp.with_icon(Arc::new(icon_data));
    }

    let options = eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    };

    eframe::run_native(
        "MassKritical",
        options,
        Box::new(move |cc| {
            if let Ok(mut guard) = egui_ctx.lock() {
                *guard = Some(cc.egui_ctx.clone());
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))?;

    Ok(())
}
