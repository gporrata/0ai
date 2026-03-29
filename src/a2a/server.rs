use anyhow::Result;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub model: Option<String>,
    pub identity: Option<String>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aMessage {
    pub session_id: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub content: Option<String>,
    pub message_type: String, // "tool_call" | "message"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aResponse {
    pub content: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct ServerState {
    pub agent_card: Arc<Mutex<AgentCard>>,
    pub message_tx: tokio::sync::mpsc::Sender<A2aMessage>,
}

pub struct A2aServer {
    pub port: u16,
    pub message_rx: Arc<Mutex<tokio::sync::mpsc::Receiver<A2aMessage>>>,
    state: ServerState,
}

impl A2aServer {
    pub fn new(agent_name: &str, model: Option<String>, identity: Option<String>) -> Result<Self> {
        let listener = TcpListener::bind("0.0.0.0:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let agent_card = AgentCard {
            name: agent_name.to_string(),
            model,
            identity,
            version: "0.1.0".to_string(),
        };

        let state = ServerState {
            agent_card: Arc::new(Mutex::new(agent_card)),
            message_tx: tx,
        };

        Ok(Self {
            port,
            message_rx: Arc::new(Mutex::new(rx)),
            state,
        })
    }

    pub fn update_model(&self, model: Option<String>) {
        let card = Arc::clone(&self.state.agent_card);
        tokio::spawn(async move {
            card.lock().await.model = model;
        });
    }

    pub async fn start(&self) -> Result<()> {
        let state = self.state.clone();
        let addr = format!("0.0.0.0:{}", self.port);

        let app = Router::new()
            .route("/a2a/info", get(get_info))
            .route("/a2a/message", post(post_message))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok(())
    }
}

async fn get_info(State(state): State<ServerState>) -> Json<AgentCard> {
    let card = state.agent_card.lock().await.clone();
    Json(card)
}

async fn post_message(
    State(state): State<ServerState>,
    Json(msg): Json<A2aMessage>,
) -> Json<A2aResponse> {
    match state.message_tx.send(msg).await {
        Ok(_) => Json(A2aResponse {
            content: "queued".to_string(),
            error: None,
        }),
        Err(e) => Json(A2aResponse {
            content: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

/// Send a tool call to a remote agent
pub async fn relay_tool_call(
    client: &reqwest::Client,
    remote_ip: &str,
    remote_port: u16,
    session_id: &str,
    tool_name: &str,
    tool_input: serde_json::Value,
) -> Result<String> {
    let url = format!("http://{}:{}/a2a/message", remote_ip, remote_port);
    let msg = A2aMessage {
        session_id: session_id.to_string(),
        tool_name: Some(tool_name.to_string()),
        tool_input: Some(tool_input),
        content: None,
        message_type: "tool_call".to_string(),
    };
    let resp: A2aResponse = client.post(&url).json(&msg).send().await?.json().await?;
    if let Some(err) = resp.error {
        return Err(anyhow::anyhow!("Remote error: {}", err));
    }
    Ok(resp.content)
}

/// Fetch agent info from remote
pub async fn fetch_agent_info(
    client: &reqwest::Client,
    remote_ip: &str,
    remote_port: u16,
) -> Result<AgentCard> {
    let url = format!("http://{}:{}/a2a/info", remote_ip, remote_port);
    let card: AgentCard = client.get(&url).send().await?.json().await?;
    Ok(card)
}
