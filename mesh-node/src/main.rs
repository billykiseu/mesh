#![windows_subsystem = "windows"]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex, mpsc as std_mpsc};
use std::time::Instant;

use anyhow::Result;
use eframe::egui;
use egui::{Color32, CornerRadius, FontId, RichText, Stroke, StrokeKind, Vec2};
use egui_extras::{TableBuilder, Column};

use mesh_core::{NodeConfig, NodeEvent, NodeHandle, MeshStats, start_mesh_node};

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
            "/help" => {
                self.push_system("Commands:".into());
                self.push_system("  /dm <name> <msg>     - Direct message".into());
                self.push_system("  /send <peer> <path>  - Send file".into());
                self.push_system("  /accept              - Accept file offer".into());
                self.push_system("  /voice <peer> <path> - Send voice file".into());
                self.push_system("  /call <peer>         - Start voice call".into());
                self.push_system("  /endcall             - End voice call".into());
                self.push_system("  /broadcast <msg>     - Public broadcast".into());
                self.push_system("  /sos <msg>           - Emergency broadcast".into());
                self.push_system("  /name <name>         - Change display name".into());
                self.push_system("  /stats               - Show mesh stats".into());
                self.push_system("  /peers               - Show peer list".into());
                self.push_system("  /nuke                - Destroy identity & exit".into());
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

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        // Convert bytes to samples
        let samples: Vec<i16> = audio_data.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        let samples = Arc::new(StdMutex::new((samples, 0usize))); // (data, position)
        let samples_clone = samples.clone();

        let stream = match device.build_output_stream(
            &config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                if let Ok(mut s) = samples_clone.lock() {
                    let (ref buf, ref mut pos) = *s;
                    for sample in data.iter_mut() {
                        if *pos < buf.len() {
                            *sample = buf[*pos];
                            *pos += 1;
                        } else {
                            *sample = 0;
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

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        // Input stream: capture 20ms frames (320 samples) and send
        let handle = self.handle.clone();
        let rt = self.rt.clone();
        let frame_buffer = Arc::new(StdMutex::new(Vec::<i16>::with_capacity(320)));
        let frame_buffer_clone = frame_buffer.clone();

        let input_stream = if let Some(input_device) = host.default_input_device() {
            match input_device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut fb) = frame_buffer_clone.lock() {
                        fb.extend_from_slice(data);
                        while fb.len() >= 320 {
                            let frame: Vec<i16> = fb.drain(..320).collect();
                            let frame_bytes: Vec<u8> = frame.iter()
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
        };

        // Output stream: plays from call_playback_buffer
        let playback_buf = self.call_playback_buffer.clone();
        let output_stream = if let Some(output_device) = host.default_output_device() {
            match output_device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    if let Ok(mut buf) = playback_buf.lock() {
                        for sample in data.iter_mut() {
                            *sample = buf.pop_front().unwrap_or(0);
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
                ui.label(RichText::new("Mesh Network").font(FontId::proportional(16.0)).color(ACCENT_CYAN).strong());
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
                for (tab, label) in [
                    (Tab::Chat, "Chat"),
                    (Tab::Peers, "Peers"),
                    (Tab::Files, "Files"),
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

        // Left panel: peer sidebar
        egui::SidePanel::left("peer_sidebar")
            .default_width(180.0)
            .min_width(120.0)
            .frame(egui::Frame::new().fill(BG_SIDEBAR).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.label(RichText::new("Peers").color(ACCENT_YELLOW).strong());
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Clone the peer data we need before the mutable borrow
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
        handle,
        rt,
        bridge_rx,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Mesh Network - {}", name))
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Mesh Network",
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
