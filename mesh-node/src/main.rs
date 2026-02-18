use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::widgets::*;

use mesh_core::{NodeConfig, NodeEvent, NodeHandle, MeshStats, start_mesh_node};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

struct ChatMessage {
    sender: String,
    content: String,
    is_system: bool,
    is_dm: bool,
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
    Stats,
}

#[derive(PartialEq)]
enum Focus {
    Input,
    Messages,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    display_name: String,
    node_id_short: String,
    port: u16,
    messages: Vec<ChatMessage>,
    peers: Vec<PeerEntry>,
    files: Vec<FileEntry>,
    input: String,
    cursor_pos: usize,
    scroll_offset: u16,
    focus: Focus,
    should_quit: bool,
    handle: NodeHandle,
    active_tab: Tab,
    peer_selected: usize,
    dm_target: Option<([u8; 32], String)>, // (node_id, display_name)
    stats: MeshStats,
    messages_relayed_display: u64,
    gateway_name: Option<String>,
}

impl App {
    fn new(
        display_name: String,
        node_id_short: String,
        port: u16,
        handle: NodeHandle,
    ) -> Self {
        Self {
            display_name,
            node_id_short,
            port,
            messages: Vec::new(),
            peers: Vec::new(),
            files: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            focus: Focus::Input,
            should_quit: false,
            handle,
            active_tab: Tab::Chat,
            peer_selected: 0,
            dm_target: None,
            stats: MeshStats::default(),
            messages_relayed_display: 0,
            gateway_name: None,
        }
    }

    fn push_system(&mut self, msg: String) {
        self.messages.push(ChatMessage {
            sender: String::new(),
            content: msg,
            is_system: true,
            is_dm: false,
        });
        self.scroll_to_bottom();
    }

    fn push_chat(&mut self, sender: String, content: String, is_dm: bool) {
        self.messages.push(ChatMessage {
            sender,
            content,
            is_system: false,
            is_dm,
        });
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = u16::MAX;
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
                    if self.peer_selected >= self.peers.len() && self.peer_selected > 0 {
                        self.peer_selected -= 1;
                    }
                } else {
                    self.push_system(format!("- {} disconnected", hex::encode(&node_id[..4])));
                }
            }
            NodeEvent::MessageReceived { sender_name, content, .. } => {
                let is_dm = true; // Direct messages through routing
                self.push_chat(sender_name, content, is_dm);
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
                self.push_system(format!("File offered by {}: {} ({}). /accept to receive", sender_name, filename, size_str));
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
            NodeEvent::VoiceReceived { sender_name, duration_ms, .. } => {
                let secs = duration_ms as f64 / 1000.0;
                self.push_system(format!("Voice note from {} ({:.1}s)", sender_name, secs));
            }
            NodeEvent::IncomingCall { peer_name, .. } => {
                self.push_system(format!("Incoming call from {}", peer_name));
            }
            NodeEvent::AudioFrame { .. } => {} // Handled by audio subsystem
            NodeEvent::CallEnded { .. } => {
                self.push_system("Call ended".into());
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
            NodeEvent::PeerList { .. } => {} // Used by FFI, not TUI directly
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

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
        match code {
            KeyCode::Esc => {
                if self.dm_target.is_some() {
                    self.dm_target = None; // Exit DM mode
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::F(1) => { self.active_tab = Tab::Chat; }
            KeyCode::F(2) => { self.active_tab = Tab::Peers; }
            KeyCode::F(3) => { self.active_tab = Tab::Files; }
            KeyCode::F(4) => {
                self.active_tab = Tab::Stats;
                // Request fresh stats
                let h = self.handle.clone();
                let _ = tokio::runtime::Handle::current().block_on(h.get_stats());
            }
            KeyCode::Tab if self.active_tab == Tab::Chat => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Messages,
                    Focus::Messages => Focus::Input,
                };
            }
            KeyCode::Up => {
                match self.active_tab {
                    Tab::Chat if self.focus == Focus::Messages => {
                        self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    }
                    Tab::Peers => {
                        self.peer_selected = self.peer_selected.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            KeyCode::Down => {
                match self.active_tab {
                    Tab::Chat if self.focus == Focus::Messages => {
                        self.scroll_offset = self.scroll_offset.saturating_add(1);
                    }
                    Tab::Peers => {
                        if !self.peers.is_empty() {
                            self.peer_selected = (self.peer_selected + 1).min(self.peers.len() - 1);
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Enter => {
                match self.active_tab {
                    Tab::Chat if self.focus == Focus::Input => {
                        if !self.input.trim().is_empty() {
                            let text = self.input.drain(..).collect::<String>();
                            self.cursor_pos = 0;
                            return Some(text);
                        }
                    }
                    Tab::Peers => {
                        // Start DM with selected peer
                        if let Some(peer) = self.peers.get(self.peer_selected) {
                            self.dm_target = Some((peer.node_id, peer.display_name.clone()));
                            self.active_tab = Tab::Chat;
                            self.focus = Focus::Input;
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) if self.focus == Focus::Input && self.active_tab == Tab::Chat => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace if self.focus == Focus::Input && self.active_tab == Tab::Chat => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete if self.focus == Focus::Input => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left if self.focus == Focus::Input => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right if self.focus == Focus::Input => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home if self.focus == Focus::Input => {
                self.cursor_pos = 0;
            }
            KeyCode::End if self.focus == Focus::Input => {
                self.cursor_pos = self.input.len();
            }
            _ => {}
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let outer = Layout::vertical([
        Constraint::Length(3),  // header
        Constraint::Length(1),  // tab bar
        Constraint::Min(5),    // body
        Constraint::Length(3), // input (only for Chat tab)
    ])
    .split(area);

    draw_header(frame, app, outer[0]);
    draw_tab_bar(frame, app, outer[1]);

    match app.active_tab {
        Tab::Chat => {
            draw_chat(frame, app, outer[2]);
            draw_input(frame, app, outer[3]);
        }
        Tab::Peers => {
            draw_peers_tab(frame, app, outer[2]);
            // Empty input area
            frame.render_widget(Block::default(), outer[3]);
        }
        Tab::Files => {
            draw_files_tab(frame, app, outer[2]);
            frame.render_widget(Block::default(), outer[3]);
        }
        Tab::Stats => {
            draw_stats_tab(frame, app, outer[2]);
            frame.render_widget(Block::default(), outer[3]);
        }
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let gw = app.gateway_name.as_deref().unwrap_or("none");
    let header_text = format!(
        " {} ({})  Peers:{}  Port:{}  GW:{}  Relayed:{}",
        app.display_name, app.node_id_short,
        app.peers.len(), app.port, gw, app.messages_relayed_display,
    );
    let header = Paragraph::new(header_text)
        .style(Style::default().fg(Color::White).bold())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Mesh Network ")
                .title_style(Style::default().fg(Color::Cyan).bold()),
        );
    frame.render_widget(header, area);
}

fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let tabs = vec!["F1:Chat", "F2:Peers", "F3:Files", "F4:Stats"];
    let tab_idx = match app.active_tab {
        Tab::Chat => 0,
        Tab::Peers => 1,
        Tab::Files => 2,
        Tab::Stats => 3,
    };

    let spans: Vec<Span> = tabs.iter().enumerate().map(|(i, t)| {
        if i == tab_idx {
            Span::styled(format!(" {} ", t), Style::default().fg(Color::Black).bg(Color::Cyan).bold())
        } else {
            Span::styled(format!(" {} ", t), Style::default().fg(Color::DarkGray))
        }
    }).collect();

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_chat(frame: &mut Frame, app: &mut App, area: Rect) {
    let body = Layout::horizontal([
        Constraint::Length(22), // peers sidebar
        Constraint::Min(30),   // messages
    ])
    .split(area);

    // Peers sidebar
    let peer_items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|p| {
            let short = hex::encode(&p.node_id[..4]);
            let gw_indicator = if p.is_gateway { " [GW]" } else { "" };
            ListItem::new(format!(" {} ({}){}", p.display_name, short, gw_indicator))
                .style(Style::default().fg(if p.is_gateway { Color::Yellow } else { Color::Green }))
        })
        .collect();

    let peers_list = List::new(peer_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Peers ")
            .title_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(peers_list, body[0]);

    // Messages area
    let msg_border_style = if app.focus == Focus::Messages {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let dm_info = app.dm_target.as_ref()
        .map(|(_, name)| format!(" DM: {} (Esc to exit) ", name))
        .unwrap_or_else(|| " Messages ".to_string());

    let msg_lines: Vec<Line> = app
        .messages
        .iter()
        .map(|m| {
            if m.is_system {
                Line::from(Span::styled(
                    format!(" * {}", m.content),
                    Style::default().fg(Color::DarkGray).italic(),
                ))
            } else if m.is_dm {
                Line::from(vec![
                    Span::styled(
                        format!(" [DM {}] ", m.sender),
                        Style::default().fg(Color::Magenta).bold(),
                    ),
                    Span::raw(&m.content),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        format!(" [{}] ", m.sender),
                        Style::default().fg(Color::Blue).bold(),
                    ),
                    Span::raw(&m.content),
                ])
            }
        })
        .collect();

    let inner_height = body[1].height.saturating_sub(2) as usize;
    let total_lines = msg_lines.len();
    let max_scroll = total_lines.saturating_sub(inner_height) as u16;
    if app.scroll_offset > max_scroll {
        app.scroll_offset = max_scroll;
    }

    let messages = Paragraph::new(msg_lines)
        .scroll((app.scroll_offset, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(msg_border_style)
                .title(dm_info)
                .title_style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(messages, body[1]);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_border_style = if app.focus == Focus::Input {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let prompt = if app.dm_target.is_some() { " DM > " } else { " > " };

    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_border_style)
            .title(prompt)
            .title_style(Style::default().fg(Color::Green)),
    );
    frame.render_widget(input, area);

    if app.focus == Focus::Input && app.active_tab == Tab::Chat {
        frame.set_cursor_position((
            area.x + 1 + app.cursor_pos as u16,
            area.y + 1,
        ));
    }
}

fn draw_peers_tab(frame: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app.peers.iter().enumerate().map(|(i, p)| {
        let short = hex::encode(&p.node_id[..4]);
        let gw = if p.is_gateway { "Yes" } else { "No" };
        let style = if i == app.peer_selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        Row::new(vec![
            p.display_name.clone(),
            short,
            gw.to_string(),
            p.bio.clone(),
        ]).style(style)
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(vec!["Name", "ID", "GW", "Bio"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Peers (Enter=DM, Up/Down=select) ")
            .title_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(table, area);
}

fn draw_files_tab(frame: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app.files.iter().map(|f| {
        let direction = if f.incoming { "IN" } else { "OUT" };
        let status = if f.done {
            "Complete".to_string()
        } else {
            format!("{}%", f.progress)
        };
        let size_str = format_size(f.size);
        Row::new(vec![
            direction.to_string(),
            f.filename.clone(),
            size_str,
            status,
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(30),
            Constraint::Length(12),
            Constraint::Min(10),
        ],
    )
    .header(
        Row::new(vec!["Dir", "Filename", "Size", "Status"])
            .style(Style::default().fg(Color::Yellow).bold())
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" File Transfers ")
            .title_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(table, area);
}

fn draw_stats_tab(frame: &mut Frame, app: &App, area: Rect) {
    let stats_text = vec![
        Line::from(vec![
            Span::styled(" Connected Peers: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.peers.len())),
        ]),
        Line::from(vec![
            Span::styled(" Messages Relayed: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.stats.messages_relayed)),
        ]),
        Line::from(vec![
            Span::styled(" Messages Received: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.stats.messages_received)),
        ]),
        Line::from(vec![
            Span::styled(" Unique Nodes Seen: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.stats.unique_nodes_seen)),
        ]),
        Line::from(vec![
            Span::styled(" Average Hops: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{:.1}", app.stats.avg_hops)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Gateway: ", Style::default().fg(Color::Yellow)),
            Span::raw(app.gateway_name.as_deref().unwrap_or("None")),
        ]),
        Line::from(vec![
            Span::styled(" Active Files: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", app.files.iter().filter(|f| !f.done).count())),
        ]),
    ];

    let para = Paragraph::new(stats_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Mesh Statistics ")
            .title_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(para, area);
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------
// Command parsing
// ---------------------------------------------------------------------------

async fn handle_command(app: &mut App, text: String) -> bool {
    let trimmed = text.trim();

    // Parse slash commands
    if trimmed.starts_with('/') {
        let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
        let cmd = parts[0].to_lowercase();

        match cmd.as_str() {
            "/dm" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let msg = parts[2];
                    if let Some(peer) = app.peers.iter().find(|p|
                        p.display_name.to_lowercase() == name.to_lowercase()
                        || hex::encode(&p.node_id[..4]) == name
                    ) {
                        let dest = peer.node_id;
                        let peer_name = peer.display_name.clone();
                        app.push_chat(format!("DM to {}", peer_name), msg.to_string(), true);
                        if let Err(e) = app.handle.send_direct(dest, msg).await {
                            app.push_system(format!("Send failed: {}", e));
                        }
                    } else {
                        app.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    app.push_system("Usage: /dm <name> <message>".into());
                }
            }
            "/send" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let path = parts[2];
                    if let Some(peer) = app.peers.iter().find(|p|
                        p.display_name.to_lowercase() == name.to_lowercase()
                        || hex::encode(&p.node_id[..4]) == name
                    ) {
                        let dest = peer.node_id;
                        app.push_system(format!("Sending file: {}", path));
                        if let Err(e) = app.handle.send_file(dest, path).await {
                            app.push_system(format!("File send failed: {}", e));
                        }
                    } else {
                        app.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    app.push_system("Usage: /send <peer> <filepath>".into());
                }
            }
            "/accept" => {
                // Accept the most recent incoming file offer
                if let Some(f) = app.files.iter().rev().find(|f| f.incoming && !f.done && f.progress == 0) {
                    let file_id = f.file_id;
                    if let Err(e) = app.handle.accept_file(file_id).await {
                        app.push_system(format!("Accept failed: {}", e));
                    } else {
                        app.push_system("File transfer accepted".into());
                    }
                } else {
                    app.push_system("No pending file offers".into());
                }
            }
            "/name" => {
                if parts.len() >= 2 {
                    let new_name = parts[1..].join(" ");
                    app.display_name = new_name.clone();
                    if let Err(e) = app.handle.update_profile(&new_name, "").await {
                        app.push_system(format!("Profile update failed: {}", e));
                    } else {
                        app.push_system(format!("Name changed to: {}", new_name));
                    }
                } else {
                    app.push_system("Usage: /name <new_name>".into());
                }
            }
            "/broadcast" => {
                if parts.len() >= 2 {
                    let msg = trimmed.strip_prefix("/broadcast ").unwrap_or("");
                    app.push_chat("[PUBLIC] You".into(), msg.to_string(), false);
                    if let Err(e) = app.handle.send_public_broadcast(msg).await {
                        app.push_system(format!("Broadcast failed: {}", e));
                    }
                } else {
                    app.push_system("Usage: /broadcast <message>".into());
                }
            }
            "/sos" => {
                if parts.len() >= 2 {
                    let msg = trimmed.strip_prefix("/sos ").unwrap_or("");
                    app.push_system(format!("!!! SOS sent: {}", msg));
                    if let Err(e) = app.handle.send_sos(msg, None).await {
                        app.push_system(format!("SOS failed: {}", e));
                    }
                } else {
                    app.push_system("Usage: /sos <message>".into());
                }
            }
            "/stats" => {
                if let Err(e) = app.handle.get_stats().await {
                    app.push_system(format!("Stats request failed: {}", e));
                }
                app.active_tab = Tab::Stats;
            }
            "/peers" => {
                app.active_tab = Tab::Peers;
            }
            "/nuke" => {
                app.push_system("Destroying identity...".into());
                if let Err(e) = app.handle.nuke().await {
                    app.push_system(format!("Nuke failed: {}", e));
                }
                return true; // Signal quit after nuke
            }
            "/voice" => {
                if parts.len() >= 3 {
                    let name = parts[1];
                    let path = parts[2];
                    if let Some(peer) = app.peers.iter().find(|p|
                        p.display_name.to_lowercase() == name.to_lowercase()
                    ) {
                        let dest = peer.node_id;
                        match std::fs::read(path) {
                            Ok(data) => {
                                let duration = (data.len() as u32 * 8) / 16; // rough estimate
                                if let Err(e) = app.handle.send_voice(Some(dest), data, duration).await {
                                    app.push_system(format!("Voice send failed: {}", e));
                                } else {
                                    app.push_system(format!("Voice note sent to {}", name));
                                }
                            }
                            Err(e) => app.push_system(format!("Failed to read file: {}", e)),
                        }
                    } else {
                        app.push_system(format!("Unknown peer: {}", name));
                    }
                } else {
                    app.push_system("Usage: /voice <peer> <filepath.opus>".into());
                }
            }
            "/help" => {
                app.push_system("Commands:".into());
                app.push_system("  /dm <name> <msg>     - Direct message".into());
                app.push_system("  /send <peer> <path>  - Send file".into());
                app.push_system("  /accept              - Accept file offer".into());
                app.push_system("  /voice <peer> <path> - Send voice file".into());
                app.push_system("  /broadcast <msg>     - Public broadcast".into());
                app.push_system("  /sos <msg>           - Emergency broadcast".into());
                app.push_system("  /name <name>         - Change display name".into());
                app.push_system("  /stats               - Show mesh stats".into());
                app.push_system("  /peers               - Show peer list".into());
                app.push_system("  /nuke                - Destroy identity & exit".into());
            }
            _ => {
                app.push_system(format!("Unknown command: {}. Type /help", cmd));
            }
        }
        return false;
    }

    // Regular message
    if let Some((dest, name)) = &app.dm_target {
        // DM mode
        let dest = *dest;
        let name = name.clone();
        app.push_chat(format!("DM to {}", name), trimmed.to_string(), true);
        if let Err(e) = app.handle.send_direct(dest, trimmed).await {
            app.push_system(format!("Send failed: {}", e));
        }
    } else {
        // Broadcast
        app.push_chat("You".into(), trimmed.to_string(), false);
        if let Err(e) = app.handle.send_broadcast(trimmed).await {
            app.push_system(format!("Send failed: {}", e));
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| {
        let hostname = whoami();
        format!("mesh-{}", &hostname[..hostname.len().min(8)])
    });
    let port: u16 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7332);

    let config = NodeConfig {
        display_name: name.clone(),
        listen_port: port,
        key_path: std::path::PathBuf::from(format!("mesh_identity_{}.key", port)),
    };

    let (identity, handle, mut event_rx) = start_mesh_node(config).await?;
    let node_id_short = identity.node_id_short();

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(name, node_id_short, port, handle);

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        tokio::select! {
            ready = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(16))) => {
                if let Ok(Ok(true)) = ready {
                    if let Ok(Event::Key(key)) = event::read() {
                        if key.kind == KeyEventKind::Press {
                            if let Some(text) = app.handle_key(key.code, key.modifiers) {
                                let quit = handle_command(&mut app, text).await;
                                if quit { break; }
                            }
                        }
                    }
                }
            }
            event = event_rx.recv() => {
                if let Some(ev) = event {
                    app.handle_mesh_event(ev);
                    while let Ok(ev) = event_rx.try_recv() {
                        app.handle_mesh_event(ev);
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn whoami() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into())
}
