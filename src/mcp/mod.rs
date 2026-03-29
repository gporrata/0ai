use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

struct McpInner {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct McpClient {
    name: String,
    inner: Arc<Mutex<McpInner>>,
    #[allow(dead_code)]
    child: Arc<Mutex<Child>>,
    tools_cache: Arc<Mutex<Vec<McpTool>>>,
}

impl McpClient {
    pub async fn connect(
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", command))?;

        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;

        let inner = Arc::new(Mutex::new(McpInner {
            stdin,
            stdout: BufReader::new(stdout),
        }));
        let child = Arc::new(Mutex::new(child));

        let client = Self {
            name: name.to_string(),
            inner,
            child,
            tools_cache: Arc::new(Mutex::new(Vec::new())),
        };

        // Initialize
        client.initialize().await?;
        // Load tools
        client.refresh_tools().await?;

        Ok(client)
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };
        let mut req_str = serde_json::to_string(&req)?;
        req_str.push('\n');

        let mut inner = self.inner.lock().await;
        inner
            .stdin
            .write_all(req_str.as_bytes())
            .await
            .context("Failed to write to MCP stdin")?;
        inner.stdin.flush().await?;

        let mut line = String::new();
        inner
            .stdout
            .read_line(&mut line)
            .await
            .context("Failed to read from MCP stdout")?;

        if line.is_empty() {
            return Err(anyhow::anyhow!("MCP server closed connection"));
        }

        let resp: JsonRpcResponse =
            serde_json::from_str(&line).context("Failed to parse MCP response")?;

        if let Some(err) = resp.error {
            return Err(anyhow::anyhow!("MCP error {}: {}", err.code, err.message));
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }

    async fn initialize(&self) -> Result<()> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "clientInfo": {
                "name": "0ai",
                "version": "0.1.0"
            }
        });
        self.send_request("initialize", params).await?;

        // Send initialized notification (no response expected)
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        let mut notif_str = serde_json::to_string(&notif)?;
        notif_str.push('\n');
        let mut inner = self.inner.lock().await;
        inner.stdin.write_all(notif_str.as_bytes()).await?;
        inner.stdin.flush().await?;

        Ok(())
    }

    async fn refresh_tools(&self) -> Result<()> {
        let result = self.send_request("tools/list", json!({})).await?;
        let tools_json = result
            .get("tools")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let tools: Vec<McpTool> = serde_json::from_value(tools_json)?;
        let mut cache = self.tools_cache.lock().await;
        *cache = tools;
        Ok(())
    }

    pub async fn list_tools(&self) -> Vec<McpTool> {
        self.tools_cache.lock().await.clone()
    }

    pub async fn call_tool(&self, tool_name: &str, input: Value) -> Result<Value> {
        let params = json!({
            "name": tool_name,
            "arguments": input
        });
        let result = self.send_request("tools/call", params).await?;
        Ok(result)
    }

    pub fn server_name(&self) -> &str {
        &self.name
    }
}

pub struct McpManager {
    pub(crate) clients: HashMap<String, McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    pub async fn connect_server(
        &mut self,
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<()> {
        let client = McpClient::connect(name, command, args, env).await?;
        self.clients.insert(name.to_string(), client);
        Ok(())
    }

    pub fn disconnect_server(&mut self, name: &str) {
        self.clients.remove(name);
    }

    pub async fn all_tools(&self) -> Vec<(String, McpTool)> {
        let mut tools = Vec::new();
        for (server_name, client) in &self.clients {
            for tool in client.list_tools().await {
                tools.push((server_name.clone(), tool));
            }
        }
        tools
    }

    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        input: Value,
    ) -> Result<Value> {
        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not connected", server_name))?;
        client.call_tool(tool_name, input).await
    }

    pub fn connected_servers(&self) -> Vec<&str> {
        self.clients.keys().map(|s| s.as_str()).collect()
    }
}
