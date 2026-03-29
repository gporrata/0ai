pub mod commands;

use crate::app::{App, AppEvent};
use crate::db::McpServer;
use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use std::io::{self, BufRead, Write};
use commands::{autocomplete, parse_input, resolve_command, ParsedInput};

// ── Chat message display ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ChatLine {
    pub role: String,
    pub content: String,
}

// ── UI State ──────────────────────────────────────────────────────────────────

pub struct Ui {
    pub chat_lines: Vec<ChatLine>,
    pub pending_shell_confirm: Option<(String, String)>, // (command, reason)
    pub response_complete: bool,
    pub shells_pending: u32,
}

impl Ui {
    pub fn new() -> Self {
        Self {
            chat_lines: Vec::new(),
            pending_shell_confirm: None,
            response_complete: false,
            shells_pending: 0,
        }
    }

    pub fn push_chat(&mut self, role: &str, content: &str) {
        self.chat_lines.push(ChatLine {
            role: role.to_string(),
            content: content.to_string(),
        });
        match role {
            "user" => println!("{} {}", style("You:").cyan().bold(), content),
            "assistant" => println!("{} {}", style("AI:").green().bold(), content),
            _ => println!("{} {}", style(">>").yellow(), content),
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !msg.is_empty() && msg != "Ready" && msg != "Error" {
            println!("{}", style(format!(".. {}", msg)).dim());
        }
    }
}

// ── Main run loop ─────────────────────────────────────────────────────────────

pub fn run(app: &mut App) -> Result<()> {
    println!(
        "{} | {} | {}",
        style("0ai").bold(),
        style(app.current_session_name()).cyan(),
        style(app.current_model_name()).green()
    );
    println!("Type /help for commands or start chatting. Ctrl+C to quit.\n");

    let stdin = io::stdin();
    loop {
        print!("{} ", style(">").bold().dim());
        io::stdout().flush()?;

        let mut input = String::new();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => {
                app.handle_bye();
                break;
            }
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }

        let input = input.trim().to_string();

        match parse_input(&input) {
            ParsedInput::Empty => continue,
            ParsedInput::Command(cmd, rest) => {
                if let Some(true) = dispatch_command(&cmd, &rest, app)? {
                    break;
                }
            }
            ParsedInput::Shell(cmd) => {
                println!("{}", style(format!("$ {}", cmd)).dim());
                let tx = app.response_tx.clone();
                let cmd_clone = cmd.clone();
                tokio::spawn(async move {
                    let (output, code) = crate::app::run_shell_command_safe(&cmd_clone).await;
                    let _ = tx
                        .send(AppEvent::ShellResult {
                            command: cmd_clone,
                            output,
                            exit_code: code,
                        })
                        .await;
                });
                wait_for_responses(app)?;
            }
            ParsedInput::Message(msg) => {
                app.ui.push_chat("user", &msg);
                app.send_message(msg);
                wait_for_responses(app)?;
            }
        }
    }

    Ok(())
}

fn wait_for_responses(app: &mut App) -> Result<()> {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.drain_responses();

        if let Some((command, reason)) = app.ui.pending_shell_confirm.take() {
            println!();
            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Run: {}\nReason: {}", command, reason))
                .default(false)
                .interact()?;
            if confirmed {
                app.ui.shells_pending += 1;
                app.ui.response_complete = false;
                let tx = app.response_tx.clone();
                let cmd = command.clone();
                tokio::spawn(async move {
                    let (output, code) = crate::app::run_shell_command_safe(&cmd).await;
                    let _ = tx
                        .send(AppEvent::ShellResult {
                            command: cmd,
                            output,
                            exit_code: code,
                        })
                        .await;
                });
            } else {
                println!("{} Shell command cancelled.", style(">>").yellow());
            }
        }

        if app.ui.response_complete && app.ui.shells_pending == 0 {
            app.ui.response_complete = false;
            break;
        }
    }
    Ok(())
}

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
            print_help();
        }
        Some("model") => {
            cmd_model_select(app)?;
        }
        Some("session") => {
            cmd_session_select(app)?;
        }
        Some("identity") => {
            app.handle_identity();
        }
        Some("marco") => {
            app.handle_marco();
        }
        Some("polo") => {
            cmd_polo_list(app)?;
        }
        Some("mcp") => {
            cmd_mcp_list(app)?;
        }
        None => {
            println!(
                "{} Unknown command '/{}'. Type /help for available commands.",
                style(">>").yellow(),
                cmd
            );
        }
        _ => {
            let matches = autocomplete(cmd);
            let names: Vec<&str> = matches.iter().map(|(c, _)| *c).collect();
            println!(
                "{} Ambiguous command '/{cmd}': {}",
                style(">>").yellow(),
                names.join(", ")
            );
        }
    }
    let _ = rest;
    Ok(Some(false))
}

fn print_help() {
    println!("\n{}", style("Available commands:").bold());
    for (cmd, desc) in commands::COMMANDS {
        println!("  {:<14} {}", style(format!("/{}", cmd)).yellow(), desc);
    }
    println!();
}

// ── Model select ──────────────────────────────────────────────────────────────

fn cmd_model_select(app: &mut App) -> Result<()> {
    let models = app.get_model_list();
    if models.is_empty() {
        println!("{} No models available.", style(">>").yellow());
        return Ok(());
    }

    let items: Vec<String> = models
        .iter()
        .map(|m| {
            if m.configured {
                format!("{} [configured]", m.display_name)
            } else {
                format!("{} [no key]", m.display_name)
            }
        })
        .collect();

    let default = models
        .iter()
        .position(|m| app.active_model_id.as_deref() == Some(&m.id))
        .unwrap_or(0);

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select model (Esc to cancel)")
        .items(&items)
        .default(default)
        .interact_opt()?;

    if let Some(i) = selection {
        let model = models[i].clone();
        if model.configured {
            app.set_active_model(&model.id, &model.provider);
            println!("{} Active model: {}", style(">>").yellow(), model.display_name);
        } else {
            let key: String = Password::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("API Key for {}", model.display_name))
                .interact()?;
            if !key.is_empty() {
                app.save_api_key(&model.id, &key);
                app.set_active_model(&model.id, &model.provider);
                println!(
                    "{} API key saved. Active model: {}",
                    style(">>").yellow(),
                    model.display_name
                );
            }
        }
    }

    Ok(())
}

// ── Session select ────────────────────────────────────────────────────────────

fn cmd_session_select(app: &mut App) -> Result<()> {
    let sessions = app.list_named_sessions();
    let mut items: Vec<String> = sessions
        .iter()
        .map(|s| s.name.as_deref().unwrap_or("?").to_string())
        .collect();
    items.push("+ New session...".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select session (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    if let Some(i) = selection {
        if i < sessions.len() {
            app.switch_to_session(sessions[i].id.clone());
        } else {
            let name: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Session name")
                .interact_text()?;
            if !name.is_empty() {
                app.create_named_session(name);
            }
        }
    }

    Ok(())
}

// ── MCP list ──────────────────────────────────────────────────────────────────

fn cmd_mcp_list(app: &mut App) -> Result<()> {
    let servers = app.list_mcp_servers();
    let mut items: Vec<String> = servers
        .iter()
        .map(|s| format!("{} — {}", s.name, s.command))
        .collect();
    items.push("+ Add MCP server...".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("MCP servers — select existing to delete, last item to add (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    if let Some(i) = selection {
        if i < servers.len() {
            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Delete '{}'?", servers[i].name))
                .default(false)
                .interact()?;
            if confirmed {
                app.delete_mcp_server(&servers[i].name);
                println!(
                    "{} Deleted MCP server: {}",
                    style(">>").yellow(),
                    servers[i].name
                );
            }
        } else {
            let entry: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("MCP entry (name command [args] [KEY=VALUE])")
                .interact_text()?;
            if !entry.is_empty() {
                parse_and_save_mcp(app, &entry);
            }
        }
    }

    Ok(())
}

// ── Polo list ─────────────────────────────────────────────────────────────────

fn cmd_polo_list(app: &mut App) -> Result<()> {
    app.refresh_discovered_agents();
    let discovered = app.discovered_agents.clone();
    let agent_sessions = app.list_agent_sessions();

    let mut items: Vec<String> = Vec::new();
    for a in &discovered {
        items.push(format!("[discovered] {} ({}:{})", a.host, a.ip, a.port));
    }
    for s in &agent_sessions {
        items.push(format!(
            "[saved] {} ({}:{})",
            s.remote_name, s.remote_host, s.remote_port
        ));
    }
    items.push("+ Manual entry...".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Agent discovery (Esc to cancel)")
        .items(&items)
        .default(0)
        .interact_opt()?;

    if let Some(i) = selection {
        if i < discovered.len() {
            let agent = discovered[i].clone();
            let name: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Session name for this agent")
                .interact_text()?;
            if !name.is_empty() {
                app.create_agent_session(name, agent.host.clone(), agent.ip.clone(), agent.port);
            }
        } else if i < discovered.len() + agent_sessions.len() {
            let session = agent_sessions[i - discovered.len()].clone();
            app.activate_agent_session(&session.id);
        }
        // last item = manual entry placeholder, nothing to do
    }

    Ok(())
}

fn parse_and_save_mcp(app: &mut App, input: &str) {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.len() < 2 {
        println!(
            "{} MCP entry requires: {{name}} {{command}} [args] [KEY=VALUE]",
            style(">>").yellow()
        );
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
    println!("{} Saved MCP server: {}", style(">>").yellow(), name);
}
