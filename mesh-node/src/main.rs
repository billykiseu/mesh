use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;
use ratatui::widgets::*;

use mesh_core::{NodeConfig, NodeEvent, NodeHandle, start_mesh_node};

/// A chat message displayed in the message panel.
struct ChatMessage {
    sender: String,
    content: String,
    is_system: bool,
}

/// Peer entry displayed in the sidebar.
struct PeerEntry {
    node_id: [u8; 32],
    display_name: String,
}

/// Focus target for keyboard navigation.
#[derive(PartialEq)]
enum Focus {
    Input,
    Messages,
}

/// Application state driving the TUI.
struct App {
    display_name: String,
    node_id_short: String,
    port: u16,
    messages: Vec<ChatMessage>,
    peers: Vec<PeerEntry>,
    input: String,
    cursor_pos: usize,
    scroll_offset: u16,
    focus: Focus,
    should_quit: bool,
    handle: NodeHandle,
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
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            focus: Focus::Input,
            should_quit: false,
            handle,
        }
    }

    fn push_system(&mut self, msg: String) {
        self.messages.push(ChatMessage {
            sender: String::new(),
            content: msg,
            is_system: true,
        });
        self.scroll_to_bottom();
    }

    fn push_chat(&mut self, sender: String, content: String) {
        self.messages.push(ChatMessage {
            sender,
            content,
            is_system: false,
        });
        self.scroll_to_bottom();
    }

    fn scroll_to_bottom(&mut self) {
        // Will be clamped during render
        self.scroll_offset = u16::MAX;
    }

    fn handle_mesh_event(&mut self, event: NodeEvent) {
        match event {
            NodeEvent::Started { node_id } => {
                self.push_system(format!("Node started ({})", &node_id[..8]));
            }
            NodeEvent::PeerConnected { node_id, display_name } => {
                self.push_system(format!("+ {} connected", display_name));
                self.peers.push(PeerEntry { node_id, display_name });
            }
            NodeEvent::PeerDisconnected { node_id } => {
                if let Some(idx) = self.peers.iter().position(|p| p.node_id == node_id) {
                    let name = self.peers.remove(idx).display_name;
                    self.push_system(format!("- {} disconnected", name));
                } else {
                    self.push_system(format!("- {} disconnected", hex::encode(&node_id[..4])));
                }
            }
            NodeEvent::MessageReceived { sender_name, content, .. } => {
                self.push_chat(sender_name, content);
            }
        }
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
        match code {
            KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Messages,
                    Focus::Messages => Focus::Input,
                };
            }
            KeyCode::Up if self.focus == Focus::Messages => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down if self.focus == Focus::Messages => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            KeyCode::Enter if self.focus == Focus::Input => {
                if !self.input.trim().is_empty() {
                    let text = self.input.drain(..).collect::<String>();
                    self.cursor_pos = 0;
                    return Some(text);
                }
            }
            KeyCode::Char(c) if self.focus == Focus::Input => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace if self.focus == Focus::Input => {
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

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Top-level vertical split: header, body, input
    let outer = Layout::vertical([
        Constraint::Length(3),  // header
        Constraint::Min(5),    // body
        Constraint::Length(3), // input
    ])
    .split(area);

    // --- Header ---
    let header_text = format!(
        " Node: {} ({})          Peers: {}  Port:{}",
        app.display_name,
        app.node_id_short,
        app.peers.len(),
        app.port,
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
    frame.render_widget(header, outer[0]);

    // --- Body: peers sidebar + messages ---
    let body = Layout::horizontal([
        Constraint::Length(22), // peers sidebar
        Constraint::Min(30),   // messages
    ])
    .split(outer[1]);

    // Peers sidebar
    let peer_items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|p| {
            let short = hex::encode(&p.node_id[..4]);
            ListItem::new(format!(" ● {} ({})", p.display_name, short))
                .style(Style::default().fg(Color::Green))
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

    let msg_lines: Vec<Line> = app
        .messages
        .iter()
        .map(|m| {
            if m.is_system {
                Line::from(Span::styled(
                    format!(" * {}", m.content),
                    Style::default().fg(Color::DarkGray).italic(),
                ))
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

    // Calculate scroll: inner height = body area height - 2 (borders)
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
                .title(" Messages ")
                .title_style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(messages, body[1]);

    // --- Input line ---
    let input_border_style = if app.focus == Focus::Input {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_border_style)
            .title(" > ")
            .title_style(Style::default().fg(Color::Green)),
    );
    frame.render_widget(input, outer[2]);

    // Place cursor
    if app.focus == Focus::Input {
        frame.set_cursor_position((
            outer[2].x + 1 + app.cursor_pos as u16,
            outer[2].y + 1,
        ));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI args
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

    // Main loop
    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Use tokio::select! to multiplex terminal events and mesh events
        tokio::select! {
            // Check for crossterm input (poll with short timeout so we don't starve mesh events)
            ready = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(16))) => {
                if let Ok(Ok(true)) = ready {
                    if let Ok(Event::Key(key)) = event::read() {
                        // On Windows, crossterm fires Press+Release; only handle Press
                        if key.kind == KeyEventKind::Press {
                            if let Some(text) = app.handle_key(key.code, key.modifiers) {
                                let trimmed = text.trim().to_string();
                                app.push_chat("You".into(), trimmed.clone());
                                let h = app.handle.clone();
                                if let Err(e) = h.send_broadcast(&trimmed).await {
                                    app.push_system(format!("Send failed: {}", e));
                                }
                            }
                        }
                    }
                }
            }
            // Drain all pending mesh events
            event = event_rx.recv() => {
                if let Some(ev) = event {
                    app.handle_mesh_event(ev);
                    // Drain any additional buffered events
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
