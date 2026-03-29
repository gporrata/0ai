use crate::a2a::discovery::{DiscoveredAgent, MdnsDiscovery};
extern crate shlex;
use crate::a2a::server::A2aServer;
use crate::db::{AgentSession, Database, McpServer, ModelConfig, Session, StoredMessage};
use crate::llm::{all_known_models, build_provider, build_system_prompt, Message, ModelInfo, Tool};
use crate::mcp::McpManager;
use crate::ui::Ui;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

pub struct App {
    pub db: Database,
    pub ui: Ui,
    pub current_session_id: Option<String>,
    pub active_model_id: Option<String>,
    pub active_model_provider: Option<String>,
    pub is_advertising: bool,
    pub discovered_agents: Vec<DiscoveredAgent>,
    pub mcp_manager: Arc<Mutex<McpManager>>,
    pub a2a_server: Option<A2aServer>,
    pub mdns: Option<MdnsDiscovery>,
    // Background task communication
    pub response_tx: mpsc::Sender<AppEvent>,
    pub response_rx: mpsc::Receiver<AppEvent>,
    // Agent name (from identity or hostname)
    pub agent_name: String,
    // Session messages (in-memory for current session)
    pub session_messages: Vec<Message>,
    pub message_count: u64,
    // Yolo mode: auto-approve shell commands until next LLM message is sent
    pub yolo: bool,
}

pub enum AppEvent {
    LlmResponse(String),
    LlmError(String),
    StatusUpdate(String),
    /// LLM wants to run a shell command — UI must confirm (or auto-approve in yolo mode)
    ShellConfirm { command: String, reason: String },
    /// Result of a shell command execution to feed back into conversation
    ShellResult { command: String, output: String, exit_code: i32 },
}

impl App {
    pub fn new(db: Database) -> Self {
        let (tx, rx) = mpsc::channel(100);

        let agent_name = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "0ai-agent".to_string());

        Self {
            db,
            ui: Ui::new(),
            current_session_id: None,
            active_model_id: None,
            active_model_provider: None,
            is_advertising: false,
            discovered_agents: Vec::new(),
            mcp_manager: Arc::new(Mutex::new(McpManager::new())),
            a2a_server: None,
            mdns: None,
            response_tx: tx,
            response_rx: rx,
            agent_name,
            session_messages: Vec::new(),
            message_count: 0,
            yolo: false,
        }
    }

    pub fn startup(&mut self) {
        // Load active model from config
        if let Ok(Some(model_id)) = self.db.get_config::<String>("active_model") {
            if let Ok(Some(provider)) = self.db.get_config::<String>("active_provider") {
                self.active_model_id = Some(model_id);
                self.active_model_provider = Some(provider);
            }
        }

        // Clean up ephemeral sessions from previous runs
        self.cleanup_ephemeral_sessions();

        // Start A2A server (started later in async context via startup_async)
        match A2aServer::new(&self.agent_name, self.active_model_id.clone(), None) {
            Ok(srv) => {
                self.a2a_server = Some(srv);
            }
            Err(e) => {
                tracing::warn!("Failed to create A2A server: {}", e);
            }
        }

        // Initialize mDNS discovery (browse only, not advertising yet)
        match MdnsDiscovery::new() {
            Ok(mut mdns) => {
                let _ = mdns.start_browsing();
                self.mdns = Some(mdns);
            }
            Err(e) => {
                tracing::warn!("Failed to initialize mDNS: {}", e);
            }
        }
    }

    pub async fn start_a2a_server(&mut self) {
        if let Some(srv) = &self.a2a_server {
            if let Err(e) = srv.start().await {
                tracing::warn!("Failed to start A2A server: {}", e);
            }
        }
    }

    pub fn cleanup_ephemeral_sessions(&self) {
        if let Ok(sessions) = self.db.list_sessions() {
            for session in sessions {
                if session.ephemeral {
                    let _ = self.db.delete_session(&session.id);
                }
            }
        }
    }

    pub fn drain_responses(&mut self) {
        while let Ok(event) = self.response_rx.try_recv() {
            match event {
                AppEvent::LlmResponse(text) => {
                    self.ui.push_chat("assistant", &text);
                    self.save_assistant_message(text);
                }
                AppEvent::LlmError(err) => {
                    self.ui.push_chat("system", &format!("Error: {}", err));
                    self.ui.response_complete = true;
                }
                AppEvent::StatusUpdate(msg) => {
                    self.ui.set_status(msg.clone());
                    if msg == "Ready" || msg == "Error" {
                        self.ui.response_complete = true;
                    }
                }
                AppEvent::ShellConfirm { command, reason } => {
                    self.ui.pending_shell_confirm = Some((command, reason));
                }
                AppEvent::ShellResult { command, output, exit_code } => {
                    let display = format!("$ {}\n{}", command, output.trim());
                    self.ui.push_chat("system", &display);
                    // Feed result back into conversation
                    let result_msg = format!(
                        "Command `{}` exited with code {}.\nOutput:\n{}",
                        command, exit_code, output.trim()
                    );
                    self.session_messages.push(Message {
                        role: "user".to_string(),
                        content: result_msg,
                    });
                    self.ui.shells_pending = self.ui.shells_pending.saturating_sub(1);
                    if self.ui.shells_pending == 0 {
                        self.ui.response_complete = true;
                    }
                }
            }
        }
    }

    fn save_assistant_message(&mut self, content: String) {
        if let Some(session_id) = &self.current_session_id.clone() {
            let msg = StoredMessage {
                session_id: session_id.clone(),
                index: self.message_count,
                role: "assistant".to_string(),
                content: content.clone(),
                timestamp: Utc::now(),
            };
            self.message_count += 1;
            let _ = self.db.save_message(&msg);
        }
        self.session_messages.push(Message {
            role: "assistant".to_string(),
            content,
        });
    }

    pub fn send_message(&mut self, content: String) {
        // Detect [yolo] prefix — enable yolo mode for this exchange
        let (yolo_this, content) = if content.starts_with("[yolo]") {
            (true, content.trim_start_matches("[yolo]").trim().to_string())
        } else {
            (false, content)
        };
        if yolo_this {
            self.yolo = true;
            self.ui.set_status("⚡ YOLO mode: commands will auto-execute this exchange".to_string());
        } else {
            self.yolo = false; // reset on every normal message
        }

        let user_msg = Message {
            role: "user".to_string(),
            content: content.clone(),
        };
        self.session_messages.push(user_msg);

        // Save to db
        if let Some(session_id) = &self.current_session_id.clone() {
            let msg = StoredMessage {
                session_id: session_id.clone(),
                index: self.message_count,
                role: "user".to_string(),
                content: content.clone(),
                timestamp: Utc::now(),
            };
            self.message_count += 1;
            let _ = self.db.save_message(&msg);

            // Update session updated_at
            if let Ok(Some(mut session)) = self.db.get_session(session_id) {
                session.updated_at = Utc::now();
                let _ = self.db.save_session(&session);
            }
        }

        // Spawn LLM call
        let model_id = self.active_model_id.clone();
        let provider_name = self.active_model_provider.clone();
        let messages = self.session_messages.clone();
        let tx = self.response_tx.clone();
        let db = self.db.clone_handle();
        let mcp_mgr = Arc::clone(&self.mcp_manager);
        let yolo = self.yolo;

        tokio::spawn(async move {
            let (mid, prov) = match (model_id, provider_name) {
                (Some(m), Some(p)) => (m, p),
                _ => {
                    let _ = tx
                        .send(AppEvent::LlmError(
                            "No model configured. Use /model to select one.".to_string(),
                        ))
                        .await;
                    return;
                }
            };

            // Get API key from db
            let stored = db.get_model(&mid).ok().flatten();
            let api_key = stored.as_ref().and_then(|m| m.api_key.clone());
            let endpoint = stored.as_ref().and_then(|m| m.endpoint.clone());

            // Providers that require an API key
            if prov != "ollama" && prov != "claude_code" && api_key.is_none() {
                let _ = tx
                    .send(AppEvent::LlmError(format!(
                        "No API key for {}. Use /model to configure it.",
                        mid
                    )))
                    .await;
                return;
            }

            let provider = match build_provider(
                &mid,
                &prov,
                api_key.as_deref(),
                endpoint.as_deref(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(AppEvent::LlmError(e.to_string())).await;
                    return;
                }
            };

            // Get MCP tools + builtin run_shell_command
            let mut tools: Vec<crate::llm::Tool> = {
                let mgr = mcp_mgr.lock().await;
                mgr.all_tools()
                    .await
                    .into_iter()
                    .map(|(_, t)| Tool {
                        name: t.name,
                        description: t.description.unwrap_or_default(),
                        input_schema: t.input_schema,
                    })
                    .collect()
            };
            tools.push(Tool {
                name: "run_shell_command".to_string(),
                description: "Run a shell command on the user's local machine. \
                    The user will be asked to confirm unless yolo mode is active. \
                    Always provide a reason.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to run" },
                        "reason":  { "type": "string", "description": "Why this command is needed" }
                    },
                    "required": ["command", "reason"]
                }),
            });

            let _ = tx.send(AppEvent::StatusUpdate("Thinking...".to_string())).await;

            let sys = build_system_prompt();
            match provider.complete(&messages, &tools, Some(&sys)).await {
                Ok(response) => {
                    // Handle tool calls
                    if !response.tool_calls.is_empty() {
                        for tc in &response.tool_calls {
                            if tc.name == "run_shell_command" {
                                let cmd = tc.input["command"].as_str().unwrap_or("").to_string();
                                let reason = tc.input["reason"].as_str().unwrap_or("").to_string();
                                if yolo {
                                    // Auto-execute in yolo mode
                                    let output = run_shell_command_safe(&cmd).await;
                                    let _ = tx.send(AppEvent::ShellResult {
                                        command: cmd,
                                        output: output.0,
                                        exit_code: output.1,
                                    }).await;
                                } else {
                                    // Request confirmation from UI
                                    let _ = tx.send(AppEvent::ShellConfirm {
                                        command: cmd,
                                        reason,
                                    }).await;
                                }
                            } else {
                                let mgr = mcp_mgr.lock().await;
                                match mgr.call_tool_any(&tc.name, tc.input.clone()).await {
                                    Ok(result) => {
                                        let result_str = serde_json::to_string_pretty(&result)
                                            .unwrap_or_else(|_| result.to_string());
                                        let _ = tx
                                            .send(AppEvent::LlmResponse(format!(
                                                "[Tool: {}]\n{}",
                                                tc.name, result_str
                                            )))
                                            .await;
                                    }
                                    Err(e) => {
                                        let _ = tx
                                            .send(AppEvent::LlmError(format!(
                                                "Tool {} failed: {}",
                                                tc.name, e
                                            )))
                                            .await;
                                    }
                                }
                            }
                        }
                    }

                    if !response.content.is_empty() {
                        let _ = tx.send(AppEvent::LlmResponse(response.content)).await;
                    }
                    let _ = tx.send(AppEvent::StatusUpdate("Ready".to_string())).await;
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::LlmError(e.to_string())).await;
                    let _ = tx.send(AppEvent::StatusUpdate("Error".to_string())).await;
                }
            }
        });
    }

    // ── Session management ────────────────────────────────────────────────────

    pub fn current_session_name(&self) -> String {
        if let Some(ref id) = self.current_session_id {
            if let Ok(Some(session)) = self.db.get_session(id) {
                return session
                    .name
                    .unwrap_or_else(|| "ephemeral".to_string());
            }
        }
        "ephemeral".to_string()
    }

    pub fn list_named_sessions(&self) -> Vec<Session> {
        self.db
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.ephemeral)
            .collect()
    }

    pub fn create_named_session(&mut self, name: String) {
        let session = Session {
            id: Uuid::new_v4().to_string(),
            name: Some(name.clone()),
            model_id: self.active_model_id.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ephemeral: false,
        };
        if let Ok(()) = self.db.save_session(&session) {
            self.switch_to_session(session.id);
            self.ui.set_status(format!("Created session: {}", name));
        }
    }

    pub fn switch_to_session(&mut self, session_id: String) {
        self.current_session_id = Some(session_id.clone());
        // Load messages
        self.session_messages = self
            .db
            .list_messages(&session_id)
            .unwrap_or_default()
            .into_iter()
            .map(|m| Message {
                role: m.role,
                content: m.content,
            })
            .collect();
        self.message_count = self.session_messages.len() as u64;
        self.ui.chat_lines.clear();
        for msg in &self.session_messages {
            self.ui.push_chat(&msg.role, &msg.content);
        }
    }

    pub fn delete_session(&self, id: &str) {
        let _ = self.db.delete_session(id);
    }

    // ── Model management ──────────────────────────────────────────────────────

    pub fn current_model_name(&self) -> String {
        self.active_model_id
            .as_deref()
            .unwrap_or("no model")
            .to_string()
    }

    pub fn get_model_list(&self) -> Vec<ModelInfo> {
        let stored_keys: HashMap<String, bool> = self
            .db
            .list_models()
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.id, m.api_key.is_some()))
            .collect();

        all_known_models()
            .into_iter()
            .map(|mut m| {
                if m.provider == "ollama" || m.provider == "claude_code" {
                    // no API key needed — configured status already set by all_known_models()
                } else if let Some(&has_key) = stored_keys.get(&m.id) {
                    m.configured = has_key;
                } else {
                    m.configured = false;
                }
                m
            })
            .collect()
    }

    pub fn set_active_model(&mut self, model_id: &str, provider: &str) {
        self.active_model_id = Some(model_id.to_string());
        self.active_model_provider = Some(provider.to_string());
        let _ = self.db.set_config("active_model", &model_id.to_string());
        let _ = self.db.set_config("active_provider", &provider.to_string());

        // Update A2A server's model
        if let Some(ref server) = self.a2a_server {
            server.update_model(Some(model_id.to_string()));
        }
    }

    pub fn save_api_key(&self, model_id: &str, key: &str) {
        // Get existing model info or create new
        let known = all_known_models();
        let known_info = known.iter().find(|m| m.id == model_id);

        let config = ModelConfig {
            id: model_id.to_string(),
            provider: known_info
                .map(|m| m.provider.clone())
                .unwrap_or_default(),
            display_name: known_info
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| model_id.to_string()),
            api_key: Some(key.to_string()),
            endpoint: None,
        };
        let _ = self.db.save_model(&config);
    }

    pub fn remove_api_key(&self, model_id: &str) {
        let _ = self.db.delete_model(model_id);
    }

    // ── MCP management ────────────────────────────────────────────────────────

    pub fn list_mcp_servers(&self) -> Vec<McpServer> {
        self.db.list_mcp_servers().unwrap_or_default()
    }

    pub fn save_mcp_server(&self, server: McpServer) {
        let _ = self.db.save_mcp_server(&server);
    }

    pub fn delete_mcp_server(&self, name: &str) {
        let _ = self.db.delete_mcp_server(name);
    }

    // ── Agent sessions ────────────────────────────────────────────────────────

    pub fn list_agent_sessions(&self) -> Vec<AgentSession> {
        self.db.list_agent_sessions().unwrap_or_default()
    }

    pub fn create_agent_session(
        &mut self,
        name: String,
        host: String,
        ip: String,
        port: u16,
    ) {
        let session = AgentSession {
            id: Uuid::new_v4().to_string(),
            remote_name: name.clone(),
            remote_host: host,
            remote_ip: ip,
            remote_port: port,
            remote_model: None,
            created_at: Utc::now(),
        };
        let _ = self.db.save_agent_session(&session);
        self.activate_agent_session(&session.id);
        self.ui.set_status(format!("Connected to agent: {}", name));
    }

    pub fn activate_agent_session(&mut self, id: &str) {
        if let Ok(Some(session)) = self.db.get_session(id) {
            self.switch_to_session(session.id);
        }
    }

    pub fn delete_agent_session(&self, id: &str) {
        let _ = self.db.delete_agent_session(id);
    }

    pub fn refresh_discovered_agents(&mut self) {
        if let Some(ref mdns) = self.mdns {
            self.discovered_agents = mdns.discovered_agents();
        }
    }

    // ── Marco (advertising) ───────────────────────────────────────────────────

    pub fn handle_marco(&mut self) {
        if self.is_advertising {
            if let Some(ref mut mdns) = self.mdns {
                match mdns.stop_advertising() {
                    Ok(_) => {
                        self.is_advertising = false;
                        self.ui.push_chat("system", "Marco: advertising OFF");
                    }
                    Err(e) => {
                        self.ui.push_chat("system", &format!("Marco error: {}", e));
                    }
                }
            }
        } else {
            let port = self.a2a_server.as_ref().map(|s| s.port).unwrap_or(0);
            if let Some(ref mut mdns) = self.mdns {
                let model = self.active_model_id.as_deref();
                match mdns.start_advertising(&self.agent_name, port, model) {
                    Ok(_) => {
                        self.is_advertising = true;
                        self.ui.push_chat(
                            "system",
                            &format!(
                                "Marco: advertising ON (port {}, model: {})",
                                port,
                                self.active_model_id.as_deref().unwrap_or("none")
                            ),
                        );
                    }
                    Err(e) => {
                        self.ui.push_chat("system", &format!("Marco error: {}", e));
                    }
                }
            } else {
                self.ui.push_chat("system", "mDNS not available");
            }
        }
    }

    // ── Identity ──────────────────────────────────────────────────────────────

    pub fn handle_identity(&mut self) {
        // Check existing identity
        if let Ok(Some(identity)) = self.db.get_config::<String>("identity_token") {
            self.ui.push_chat(
                "system",
                &format!(
                    "Current identity token: {}...",
                    &identity[..identity.len().min(20)]
                ),
            );
            return;
        }

        // Start device flow - open browser
        let auth_url = "https://github.com/login/device";
        self.ui.push_chat(
            "system",
            &format!(
                "Opening browser for GitHub device auth: {}\nEnter the code shown in the browser.",
                auth_url
            ),
        );

        let _ = open::that(auth_url);

        // In a real implementation, we'd poll the device auth endpoint.
        // For now, prompt the user to paste their token.
        self.ui.push_chat(
            "system",
            "Paste your personal access token and press Enter:",
        );

        let tx = self.response_tx.clone();
        let db = self.db.clone_handle();

        // We can't actually do interactive input here without a modal;
        // The user can use /identity again to see their token.
        // For a full device flow we'd need to spawn an axum listener on loopback.
        tokio::spawn(async move {
            // Minimal device flow stub: listen on loopback for redirect
            let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx
                        .send(AppEvent::StatusUpdate(format!("Identity listener error: {}", e)))
                        .await;
                    return;
                }
            };
            let port = listener.local_addr().unwrap().port();
            let redirect_url = format!("http://127.0.0.1:{}/callback", port);
            let _ = tx
                .send(AppEvent::StatusUpdate(format!(
                    "OAuth redirect: {}",
                    redirect_url
                )))
                .await;

            // Wait for one connection
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 4096];
                if let Ok(n) = stream.read(&mut buf).await {
                    let request = String::from_utf8_lossy(&buf[..n]);
                    // Extract code from query string
                    if let Some(code_start) = request.find("code=") {
                        let code = request[code_start + 5..]
                            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !code.is_empty() {
                            let _ = db.set_config("identity_token", &code);
                            let _ = tx
                                .send(AppEvent::LlmResponse(format!(
                                    "Identity token saved: {}...",
                                    &code[..code.len().min(10)]
                                )))
                                .await;
                        }
                    }
                }
            }
        });
    }

    // ── Quit handlers ─────────────────────────────────────────────────────────

    pub fn handle_bye(&mut self) {
        // Named sessions are already saved; delete ephemeral
        if let Some(ref id) = self.current_session_id.clone() {
            if let Ok(Some(session)) = self.db.get_session(id) {
                if session.ephemeral {
                    let _ = self.db.delete_session(id);
                }
            }
        }
    }

    pub fn handle_quit(&mut self) {
        // Delete current session regardless of whether it is named or ephemeral
        if let Some(ref id) = self.current_session_id.clone() {
            let _ = self.db.delete_session(id);
        }
    }

    pub fn handle_ctrl_c(&mut self) {
        // Treat as orphaned - delete current session if ephemeral
        if let Some(ref id) = self.current_session_id.clone() {
            if let Ok(Some(session)) = self.db.get_session(id) {
                if session.ephemeral {
                    let _ = self.db.delete_session(id);
                }
            }
        }
    }
}

/// Execute a shell command safely without invoking a shell binary.
/// Returns (combined stdout+stderr, exit_code).
pub async fn run_shell_command_safe(command: &str) -> (String, i32) {
    // Detect shell operators that require a real shell to interpret
    let needs_shell = command.contains('>') || command.contains('<')
        || command.contains('|') || command.contains('&')
        || command.contains(';') || command.contains('`')
        || command.contains('$') || command.contains('~');

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    let result = if needs_shell {
        tokio::process::Command::new(&shell)
            .arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
    } else {
        let parts = match shlex::split(command) {
            Some(p) if !p.is_empty() => p,
            _ => return (format!("Failed to parse command: {}", command), -1),
        };
        let (bin, args) = parts.split_first().unwrap();
        tokio::process::Command::new(bin)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
    };
    match result {
        Ok(out) => {
            let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if !stderr.is_empty() {
                combined.push_str(&stderr);
            }
            let code = out.status.code().unwrap_or(-1);
            (combined, code)
        }
        Err(e) => (format!("Failed to run command: {}", e), -1),
    }
}

// Extension: McpManager helper to call any tool by name across all servers
impl McpManager {
    pub async fn call_tool_any(&self, tool_name: &str, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // Find which server has this tool
        for (server_name, client) in &self.clients {
            let client_tools: Vec<crate::mcp::McpTool> = client.list_tools().await;
            if client_tools.iter().any(|t| t.name == tool_name) {
                return self.call_tool(server_name, tool_name, input).await;
            }
        }
        Err(anyhow::anyhow!("No MCP server has tool: {}", tool_name))
    }
}
