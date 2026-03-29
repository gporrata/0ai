pub mod commands;

use crate::app::App;
use crate::db::McpServer;
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
    },
    Frame, Terminal,
};
use std::io;
use commands::{autocomplete, parse_input, resolve_command, ParsedInput};

// ── Chat message display ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ChatLine {
    pub role: String, // "user", "assistant", "system"
    pub content: String,
}

// ── Modal states ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    None,
    ModelSelect,
    SessionSelect,
    McpList,
    PoloList,
    InputPrompt(InputPromptKind),
    ApiKeyPrompt(String), // model_id
    Help,
    ShellConfirm { command: String, reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputPromptKind {
    NewSessionName,
    McpEntry,
    AgentSessionName(String, String, u16), // host, ip, port
}

// ── Full UI state ─────────────────────────────────────────────────────────────

pub struct Ui {
    pub input: String,
    pub cursor_pos: usize,
    pub chat_lines: Vec<ChatLine>,
    pub modal: Modal,
    pub list_state: ListState,
    pub prompt_input: String,
    pub status_message: Option<String>,
    pub scroll_offset: usize,
}

impl Ui {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            chat_lines: Vec::new(),
            modal: Modal::None,
            list_state: ListState::default(),
            prompt_input: String::new(),
            status_message: None,
            scroll_offset: 0,
        }
    }

    pub fn push_chat(&mut self, role: &str, content: &str) {
        self.chat_lines.push(ChatLine {
            role: role.to_string(),
            content: content.to_string(),
        });
        self.scroll_offset = 0; // scroll to bottom
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
    }

    fn input_push(&mut self, ch: char) {
        self.input.insert(self.cursor_pos, ch);
        self.cursor_pos += ch.len_utf8();
    }

    fn input_backspace(&mut self) {
        if self.cursor_pos > 0 {
            let start = self
                .input
                .char_indices()
                .rev()
                .find(|(i, _)| *i < self.cursor_pos)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(start);
            self.cursor_pos = start;
        }
    }
}

// ── Main TUI run loop ─────────────────────────────────────────────────────────

pub fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    app.ui.push_chat(
        "system",
        "Welcome to 0ai! Type /help for commands or start chatting.",
    );

    loop {
        terminal.draw(|f| render(f, app))?;

        if let Ok(true) = event::poll(std::time::Duration::from_millis(100)) {
            if let Ok(evt) = event::read() {
                match evt {
                    Event::Key(key) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c')
                        {
                            // Ctrl+C: treat as orphaned, delete ephemeral
                            app.handle_ctrl_c();
                            return Ok(());
                        }

                        let should_quit = handle_key(key, app)?;
                        if should_quit {
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        }

        // Drain async results from background tasks
        app.drain_responses();
    }
}

/// Returns true if we should quit
fn handle_key(key: KeyEvent, app: &mut App) -> Result<bool> {
    match &app.ui.modal.clone() {
        Modal::None => handle_key_main(key, app),
        Modal::ModelSelect => {
            handle_key_model_select(key, app)?;
            Ok(false)
        }
        Modal::SessionSelect => {
            handle_key_session_select(key, app)?;
            Ok(false)
        }
        Modal::McpList => {
            handle_key_mcp_list(key, app)?;
            Ok(false)
        }
        Modal::PoloList => {
            handle_key_polo_list(key, app)?;
            Ok(false)
        }
        Modal::InputPrompt(kind) => {
            let kind = kind.clone();
            handle_key_input_prompt(key, app, kind)?;
            Ok(false)
        }
        Modal::ApiKeyPrompt(model_id) => {
            let model_id = model_id.clone();
            handle_key_api_key_prompt(key, app, &model_id)?;
            Ok(false)
        }
        Modal::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')
            ) {
                app.ui.modal = Modal::None;
            }
            Ok(false)
        }
        Modal::ShellConfirm { command, reason: _ } => {
            let command = command.clone();
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    app.ui.modal = Modal::None;
                    let tx = app.response_tx.clone();
                    tokio::spawn(async move {
                        let output = crate::app::run_shell_command_safe(&command).await;
                        let _ = tx.send(crate::app::AppEvent::ShellResult {
                            command,
                            output: output.0,
                            exit_code: output.1,
                        }).await;
                    });
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    app.ui.push_chat("system", "Shell command cancelled.");
                    app.ui.modal = Modal::None;
                }
                _ => {}
            }
            Ok(false)
        }
    }
}

fn handle_key_main(key: KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Char(c) => {
            app.ui.input_push(c);
        }
        KeyCode::Backspace => {
            app.ui.input_backspace();
        }
        KeyCode::Left => {
            if app.ui.cursor_pos > 0 {
                app.ui.cursor_pos -= 1;
            }
        }
        KeyCode::Right => {
            if app.ui.cursor_pos < app.ui.input.len() {
                app.ui.cursor_pos += 1;
            }
        }
        KeyCode::Up => {
            if app.ui.scroll_offset + 1 < app.ui.chat_lines.len() {
                app.ui.scroll_offset += 1;
            }
        }
        KeyCode::Down => {
            if app.ui.scroll_offset > 0 {
                app.ui.scroll_offset -= 1;
            }
        }
        KeyCode::Tab => {
            // Autocomplete: if typing a command, complete the first match
            if app.ui.input.starts_with('/') {
                let partial = &app.ui.input[1..];
                let matches = autocomplete(partial);
                if let Some((cmd, _)) = matches.first() {
                    app.ui.input = format!("/{}", cmd);
                    app.ui.cursor_pos = app.ui.input.len();
                }
            }
        }
        KeyCode::Enter => {
            let input = app.ui.input.trim().to_string();
            if input.is_empty() {
                return Ok(false);
            }
            app.ui.input.clear();
            app.ui.cursor_pos = 0;

            match parse_input(&input) {
                ParsedInput::Empty => {}
                ParsedInput::Command(cmd, rest) => {
                    if let Some(quit) = dispatch_command(&cmd, &rest, app)? {
                        return Ok(quit);
                    }
                }
                ParsedInput::Shell(cmd) => {
                    app.ui.push_chat("system", &format!("$ {}", cmd));
                    let tx = app.response_tx.clone();
                    tokio::spawn(async move {
                        let (output, code) = crate::app::run_shell_command_safe(&cmd).await;
                        let _ = tx.send(crate::app::AppEvent::ShellResult {
                            command: cmd,
                            output,
                            exit_code: code,
                        }).await;
                    });
                }
                ParsedInput::Message(msg) => {
                    app.ui.push_chat("user", &msg);
                    app.send_message(msg);
                }
            }
        }
        KeyCode::Esc => {
            app.ui.input.clear();
            app.ui.cursor_pos = 0;
        }
        _ => {}
    }
    Ok(false)
}

/// Dispatch a /command. Returns Some(true) to quit, Some(false) to continue, None if unrecognized.
fn dispatch_command(cmd: &str, rest: &str, app: &mut App) -> Result<Option<bool>> {
    let resolved = resolve_command(cmd);
    match resolved {
        Some("bye") => {
            app.handle_bye();
            return Ok(Some(true));
        }
        Some("quit") => {
            app.handle_quit();
            return Ok(Some(true));
        }
        Some("help") => {
            app.ui.modal = Modal::Help;
        }
        Some("model") => {
            open_model_select(app);
        }
        Some("session") => {
            open_session_select(app);
        }
        Some("identity") => {
            app.handle_identity();
        }
        Some("marco") => {
            app.handle_marco();
        }
        Some("polo") => {
            open_polo_list(app);
        }
        Some("mcp") => {
            open_mcp_list(app);
        }
        None => {
            app.ui.push_chat(
                "system",
                &format!(
                    "Unknown command '/{}'. Type /help for available commands.",
                    cmd
                ),
            );
        }
        _ => {
            // ambiguous
            let matches = autocomplete(cmd);
            let names: Vec<&str> = matches.iter().map(|(c, _)| *c).collect();
            app.ui.push_chat(
                "system",
                &format!("Ambiguous command '/{cmd}': {}", names.join(", ")),
            );
        }
    }
    let _ = rest; // suppress warning
    Ok(Some(false))
}

// ── Model select ──────────────────────────────────────────────────────────────

fn open_model_select(app: &mut App) {
    app.ui.modal = Modal::ModelSelect;
    app.ui.list_state = ListState::default();
    app.ui.list_state.select(Some(0));
}

fn handle_key_model_select(key: KeyEvent, app: &mut App) -> Result<()> {
    let models = app.get_model_list();
    let len = models.len();
    if len == 0 {
        app.ui.modal = Modal::None;
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
        }
        KeyCode::Up => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some((i + 1).min(len - 1)));
        }
        KeyCode::Delete => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < models.len() {
                    let model = &models[i];
                    app.remove_api_key(&model.id);
                    app.ui.set_status(format!("Removed API key for {}", model.display_name));
                }
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < models.len() {
                    let model = models[i].clone();
                    if model.configured {
                        app.set_active_model(&model.id, &model.provider);
                        app.ui.set_status(format!("Active model: {}", model.display_name));
                        app.ui.modal = Modal::None;
                    } else {
                        app.ui.modal = Modal::ApiKeyPrompt(model.id.clone());
                        app.ui.prompt_input.clear();
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Session select ────────────────────────────────────────────────────────────

fn open_session_select(app: &mut App) {
    app.ui.modal = Modal::SessionSelect;
    app.ui.list_state = ListState::default();
    app.ui.list_state.select(Some(0));
}

fn handle_key_session_select(key: KeyEvent, app: &mut App) -> Result<()> {
    let sessions = app.list_named_sessions();
    let len = sessions.len() + 1; // +1 for "new session"
    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
        }
        KeyCode::Up => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some((i + 1).min(len - 1)));
        }
        KeyCode::Delete => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < sessions.len() {
                    let session = sessions[i].clone();
                    app.delete_session(&session.id);
                    app.ui.set_status(format!(
                        "Deleted session: {}",
                        session.name.as_deref().unwrap_or("?")
                    ));
                    let new_len = sessions.len(); // after deletion
                    if new_len > 0 {
                        app.ui
                            .list_state
                            .select(Some(i.min(new_len.saturating_sub(1))));
                    }
                }
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < sessions.len() {
                    let session = sessions[i].clone();
                    app.switch_to_session(session.id);
                    app.ui.modal = Modal::None;
                } else {
                    // "new session" entry
                    app.ui.modal = Modal::InputPrompt(InputPromptKind::NewSessionName);
                    app.ui.prompt_input.clear();
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ── MCP list ──────────────────────────────────────────────────────────────────

fn open_mcp_list(app: &mut App) {
    app.ui.modal = Modal::McpList;
    app.ui.list_state = ListState::default();
    app.ui.list_state.select(Some(0));
}

fn handle_key_mcp_list(key: KeyEvent, app: &mut App) -> Result<()> {
    let servers = app.list_mcp_servers();
    let len = servers.len() + 1; // +1 for "new entry"
    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
        }
        KeyCode::Up => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some((i + 1).min(len - 1)));
        }
        KeyCode::Delete => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < servers.len() {
                    let name = servers[i].name.clone();
                    app.delete_mcp_server(&name);
                    app.ui.set_status(format!("Deleted MCP server: {}", name));
                }
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.ui.list_state.selected() {
                if i == servers.len() {
                    // "new entry"
                    app.ui.modal = Modal::InputPrompt(InputPromptKind::McpEntry);
                    app.ui.prompt_input.clear();
                }
                // selecting existing does nothing
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Polo list ─────────────────────────────────────────────────────────────────

fn open_polo_list(app: &mut App) {
    // Refresh discovery
    app.refresh_discovered_agents();
    app.ui.modal = Modal::PoloList;
    app.ui.list_state = ListState::default();
    app.ui.list_state.select(Some(0));
}

fn handle_key_polo_list(key: KeyEvent, app: &mut App) -> Result<()> {
    let discovered = app.discovered_agents.clone();
    let agent_sessions = app.list_agent_sessions();
    // Layout: discovered agents, then existing agent sessions, then "new session"
    let total = discovered.len() + agent_sessions.len() + 1;

    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
        }
        KeyCode::Up => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down => {
            let i = app.ui.list_state.selected().unwrap_or(0);
            app.ui.list_state.select(Some((i + 1).min(total.saturating_sub(1))));
        }
        KeyCode::Delete => {
            if let Some(i) = app.ui.list_state.selected() {
                let offset = discovered.len();
                if i >= offset && i < offset + agent_sessions.len() {
                    let session = agent_sessions[i - offset].clone();
                    app.delete_agent_session(&session.id);
                    app.ui
                        .set_status(format!("Deleted agent session: {}", session.remote_name));
                }
            }
        }
        KeyCode::Enter => {
            if let Some(i) = app.ui.list_state.selected() {
                if i < discovered.len() {
                    // Start session with discovered agent
                    let agent = discovered[i].clone();
                    app.ui.modal = Modal::InputPrompt(InputPromptKind::AgentSessionName(
                        agent.host.clone(),
                        agent.ip.clone(),
                        agent.port,
                    ));
                    app.ui.prompt_input.clear();
                } else {
                    let offset = discovered.len();
                    if i < offset + agent_sessions.len() {
                        let session = agent_sessions[i - offset].clone();
                        app.activate_agent_session(&session.id);
                        app.ui.modal = Modal::None;
                    }
                    // last entry = "new session" (no extra action needed)
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Input prompt ──────────────────────────────────────────────────────────────

fn handle_key_input_prompt(key: KeyEvent, app: &mut App, kind: InputPromptKind) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
            app.ui.prompt_input.clear();
        }
        KeyCode::Backspace => {
            app.ui.prompt_input.pop();
        }
        KeyCode::Char(c) => {
            app.ui.prompt_input.push(c);
        }
        KeyCode::Enter => {
            let value = app.ui.prompt_input.trim().to_string();
            app.ui.prompt_input.clear();
            match kind {
                InputPromptKind::NewSessionName => {
                    if !value.is_empty() {
                        app.create_named_session(value);
                    }
                    app.ui.modal = Modal::None;
                }
                InputPromptKind::McpEntry => {
                    // Format: {name} {command} {args?} {env KEY=VALUE...}
                    if !value.is_empty() {
                        parse_and_save_mcp(app, &value);
                    }
                    app.ui.modal = Modal::None;
                }
                InputPromptKind::AgentSessionName(host, ip, port) => {
                    if !value.is_empty() {
                        app.create_agent_session(value, host, ip, port);
                    }
                    app.ui.modal = Modal::None;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn parse_and_save_mcp(app: &mut App, input: &str) {
    // Format: name command [args...] [KEY=VALUE...]
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.len() < 2 {
        app.ui.set_status("MCP entry requires: {name} {command} [args] [KEY=VALUE]");
        return;
    }
    let name = tokens[0].to_string();
    let command = tokens[1].to_string();

    let mut args = Vec::new();
    let mut env = std::collections::HashMap::new();

    for token in &tokens[2..] {
        if token.contains('=') {
            let parts: Vec<&str> = token.splitn(2, '=').collect();
            if parts.len() == 2 {
                env.insert(parts[0].to_string(), parts[1].to_string());
            }
        } else {
            args.push(token.to_string());
        }
    }

    let server = McpServer {
        name: name.clone(),
        command,
        args,
        env,
    };
    app.save_mcp_server(server);
    app.ui.set_status(format!("Saved MCP server: {}", name));
}

// ── API key prompt ────────────────────────────────────────────────────────────

fn handle_key_api_key_prompt(key: KeyEvent, app: &mut App, model_id: &str) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.ui.modal = Modal::None;
            app.ui.prompt_input.clear();
        }
        KeyCode::Backspace => {
            app.ui.prompt_input.pop();
        }
        KeyCode::Char(c) => {
            app.ui.prompt_input.push(c);
        }
        KeyCode::Enter => {
            let key_value = app.ui.prompt_input.trim().to_string();
            app.ui.prompt_input.clear();
            if !key_value.is_empty() {
                app.save_api_key(model_id, &key_value);
                // Now set it as active
                let models = app.get_model_list();
                if let Some(m) = models.iter().find(|m| m.id == model_id) {
                    app.set_active_model(&m.id, &m.provider);
                    app.ui.set_status(format!("API key saved. Active model: {}", m.display_name));
                }
            }
            app.ui.modal = Modal::None;
        }
        _ => {}
    }
    Ok(())
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Layout: chat area, status bar, input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(size);

    render_chat(f, app, chunks[0]);
    render_status(f, app, chunks[1]);
    render_input(f, app, chunks[2]);

    // Overlay modals
    match &app.ui.modal.clone() {
        Modal::None => {}
        Modal::Help => render_help(f, size),
        Modal::ModelSelect => render_model_select(f, app, size),
        Modal::SessionSelect => render_session_select(f, app, size),
        Modal::McpList => render_mcp_list(f, app, size),
        Modal::PoloList => render_polo_list(f, app, size),
        Modal::InputPrompt(kind) => {
            let title = match kind {
                InputPromptKind::NewSessionName => "New Session Name",
                InputPromptKind::McpEntry => "MCP Entry (name command [args] [KEY=VALUE])",
                InputPromptKind::AgentSessionName(..) => "Agent Session Name",
            };
            render_input_prompt(f, app, title, false, size);
        }
        Modal::ApiKeyPrompt(model_id) => {
            let title = format!("API Key for {} (input hidden)", model_id);
            render_input_prompt(f, app, &title, true, size);
        }
        Modal::ShellConfirm { command, reason } => {
            render_shell_confirm(f, command, reason, size);
        }
    }

    // Autocomplete popup when typing a command
    if app.ui.modal == Modal::None && app.ui.input.starts_with('/') {
        let partial = &app.ui.input[1..];
        let partial_no_space = partial.split_whitespace().next().unwrap_or(partial);
        if !partial_no_space.is_empty() && !partial.contains(' ') {
            let matches = autocomplete(partial_no_space);
            if !matches.is_empty() {
                render_autocomplete(f, &matches, chunks[2]);
            }
        }
    }
}

/// Word-wrap `text` to `width` columns, returning one string per display row.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut rows = Vec::new();
    for segment in text.split('\n') {
        if segment.is_empty() {
            rows.push(String::new());
            continue;
        }
        let mut remaining = segment;
        while !remaining.is_empty() {
            if remaining.chars().count() <= width {
                rows.push(remaining.to_string());
                break;
            }
            // Find the last space within `width` chars
            let byte_boundary = remaining
                .char_indices()
                .take(width + 1)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(remaining.len());
            let slice = &remaining[..byte_boundary];
            let split = slice.rfind(' ').unwrap_or(byte_boundary);
            rows.push(remaining[..split].trim_end().to_string());
            remaining = remaining[split..].trim_start_matches(' ');
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn render_chat(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0, 0, 77)))
        .title_style(Style::default().fg(Color::Rgb(255, 255, 204)))
        .title(format!(
            " 0ai | {} | {} ",
            app.current_session_name(),
            app.current_model_name()
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width as usize;
    let prefix_len = 5usize; // "You: " / "AI:  " / "  >> "

    // Expand each chat message into wrapped display rows
    let mut display: Vec<(String, Color)> = Vec::new();
    for line in &app.ui.chat_lines {
        let (prefix, color) = match line.role.as_str() {
            "user" => ("You: ", Color::Cyan),
            "assistant" => ("AI:  ", Color::Green),
            _ => ("  >> ", Color::Yellow),
        };
        let first_line = format!("{}{}", prefix, line.content);
        let indent = " ".repeat(prefix_len);
        let rows = word_wrap(&first_line, width);
        for (i, row) in rows.into_iter().enumerate() {
            if i == 0 {
                display.push((row, color));
            } else {
                display.push((format!("{}{}", indent, row), color));
            }
        }
    }

    let total = display.len();
    let visible_height = inner.height as usize;
    let scroll = app.ui.scroll_offset;
    let start = if total > visible_height {
        (total - visible_height).saturating_sub(scroll)
    } else {
        0
    };
    let end = (start + visible_height).min(total);

    let list_items: Vec<ListItem> = display[start..end]
        .iter()
        .map(|(text, color)| {
            ListItem::new(Line::from(Span::styled(
                text.clone(),
                Style::default().fg(*color),
            )))
        })
        .collect();

    f.render_widget(List::new(list_items), inner);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let marco_status = if app.is_advertising {
        " [MARCO:ON]"
    } else {
        ""
    };
    let status = app
        .ui
        .status_message
        .as_deref()
        .unwrap_or("Ready")
        .to_string()
        + marco_status;
    let para = Paragraph::new(status).style(Style::default().fg(Color::DarkGray));
    f.render_widget(para, area);
}

fn render_input(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0, 0, 77)))
        .title_style(Style::default().fg(Color::Rgb(255, 255, 204)))
        .title(" Input ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let display_text = if app.ui.input.is_empty() {
        Span::styled(
            "Type a message or /command...",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(&app.ui.input, Style::default().fg(Color::White))
    };
    let para = Paragraph::new(Line::from(vec![display_text]));
    f.render_widget(para, inner);

    // Show cursor
    let cursor_x = inner.x + (app.ui.cursor_pos as u16).min(inner.width.saturating_sub(1));
    f.set_cursor_position((cursor_x, inner.y));
}

fn render_autocomplete(f: &mut Frame, matches: &[(&str, &str)], input_area: Rect) {
    let height = (matches.len() as u16 + 2).min(10);
    let width = 50u16.min(input_area.width);
    let y = input_area.y.saturating_sub(height);
    let popup_area = Rect {
        x: input_area.x,
        y,
        width,
        height,
    };
    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = matches
        .iter()
        .map(|(cmd, desc)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("/{:<12}", cmd),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {}", desc), Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Commands "),
    );
    f.render_widget(list, popup_area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 80, area);
    f.render_widget(Clear, popup);

    let lines: Vec<Line> = commands::COMMANDS
        .iter()
        .map(|(cmd, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("  /{:<12}", cmd),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" - {}", desc), Style::default().fg(Color::White)),
            ])
        })
        .collect();

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help - ESC to close "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, popup);
}

fn render_model_select(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rect(60, 80, area);
    f.render_widget(Clear, popup);

    let models = app.get_model_list();
    let items: Vec<ListItem> = models
        .iter()
        .map(|m| {
            let status = if m.configured { "[configured]" } else { "[needs key]" };
            let is_active = app
                .active_model_id
                .as_deref()
                .map(|id| id == m.id)
                .unwrap_or(false);
            let active_mark = if is_active { "* " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{}{}", active_mark, m.display_name),
                    Style::default().fg(if m.configured {
                        Color::Green
                    } else {
                        Color::Gray
                    }),
                ),
                Span::styled(
                    format!("  {}", status),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Model - ENTER select | DEL remove key | ESC cancel "),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    f.render_stateful_widget(list, popup, &mut app.ui.list_state);
}

fn render_session_select(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rect(60, 70, area);
    f.render_widget(Clear, popup);

    let sessions = app.list_named_sessions();
    let mut items: Vec<ListItem> = sessions
        .iter()
        .map(|s| {
            let name = s.name.as_deref().unwrap_or("unnamed");
            let active = app.current_session_id.as_deref() == Some(s.id.as_str());
            let mark = if active { "* " } else { "  " };
            ListItem::new(Span::styled(
                format!("{}{}", mark, name),
                Style::default().fg(Color::Cyan),
            ))
        })
        .collect();

    items.push(ListItem::new(Span::styled(
        "  + New session...",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Sessions - ENTER select | DEL delete | ESC cancel "),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    f.render_stateful_widget(list, popup, &mut app.ui.list_state);
}

fn render_mcp_list(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rect(70, 70, area);
    f.render_widget(Clear, popup);

    let servers = app.list_mcp_servers();
    let mut items: Vec<ListItem> = servers
        .iter()
        .map(|s| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {} ", s.name),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("=> {} {}", s.command, s.args.join(" ")),
                    Style::default().fg(Color::Gray),
                ),
            ]))
        })
        .collect();

    items.push(ListItem::new(Span::styled(
        "  + New MCP server...",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" MCP Servers - ENTER (new) | DEL delete | ESC cancel "),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    f.render_stateful_widget(list, popup, &mut app.ui.list_state);
}

fn render_polo_list(f: &mut Frame, app: &mut App, area: Rect) {
    let popup = centered_rect(70, 80, area);
    f.render_widget(Clear, popup);

    let discovered = app.discovered_agents.clone();
    let agent_sessions = app.list_agent_sessions();

    let mut items: Vec<ListItem> = Vec::new();

    if !discovered.is_empty() {
        items.push(ListItem::new(Span::styled(
            " -- Discovered on network -- ",
            Style::default().fg(Color::Yellow),
        )));
        for agent in &discovered {
            let model = agent.model.as_deref().unwrap_or("unknown");
            items.push(ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {} ", agent.name),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}:{} [{}]", agent.ip, agent.port, model),
                    Style::default().fg(Color::Gray),
                ),
            ])));
        }
    }

    if !agent_sessions.is_empty() {
        items.push(ListItem::new(Span::styled(
            " -- Agent sessions -- ",
            Style::default().fg(Color::Yellow),
        )));
        for s in &agent_sessions {
            items.push(ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {} ", s.remote_name),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "{}:{} [{}]",
                        s.remote_ip,
                        s.remote_port,
                        s.remote_model.as_deref().unwrap_or("?")
                    ),
                    Style::default().fg(Color::Gray),
                ),
            ])));
        }
    }

    items.push(ListItem::new(Span::styled(
        "  + Manually specify agent...",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Polo - ENTER connect | DEL delete session | ESC cancel "),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    f.render_stateful_widget(list, popup, &mut app.ui.list_state);
}

fn render_input_prompt(f: &mut Frame, app: &App, title: &str, _masked: bool, area: Rect) {
    let popup = centered_rect(60, 20, area);
    f.render_widget(Clear, popup);

    let display = if _masked {
        "*".repeat(app.ui.prompt_input.len())
    } else {
        app.ui.prompt_input.clone()
    };

    let para = Paragraph::new(display)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} - ENTER confirm | ESC cancel ", title)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(para, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_shell_confirm(f: &mut Frame, command: &str, reason: &str, area: Rect) {
    let popup = centered_rect(70, 30, area);
    f.render_widget(Clear, popup);

    let content = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Reason: ", Style::default().fg(Color::DarkGray)),
            Span::styled(reason, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  $ ", Style::default().fg(Color::Green)),
            Span::styled(command, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [Enter/y] Run    [Esc/n] Cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];

    let para = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(0, 0, 77)))
                .title_style(Style::default().fg(Color::Rgb(255, 255, 204)))
                .title(" Run command? "),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(para, popup);
}
