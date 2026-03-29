use anyhow::{Context, Result};
use redb::{Database as RedbDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// Table definitions
pub const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");
pub const CONFIG: TableDefinition<&str, &[u8]> = TableDefinition::new("config");
pub const MODELS: TableDefinition<&str, &[u8]> = TableDefinition::new("models");
pub const MCP_SERVERS: TableDefinition<&str, &[u8]> = TableDefinition::new("mcp_servers");
pub const AGENT_SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("agent_sessions");
pub const MESSAGES: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("messages");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    pub model_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub remote_name: String,
    pub remote_host: String,
    pub remote_ip: String,
    pub remote_port: u16,
    pub remote_model: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub session_id: String,
    pub index: u64,
    pub role: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct Database {
    inner: Arc<Mutex<RedbDatabase>>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let db = RedbDatabase::create(path).context("Failed to open database")?;
        let db = Self {
            inner: Arc::new(Mutex::new(db)),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            write_txn.open_table(SESSIONS)?;
            write_txn.open_table(CONFIG)?;
            write_txn.open_table(MODELS)?;
            write_txn.open_table(MCP_SERVERS)?;
            write_txn.open_table(AGENT_SESSIONS)?;
            write_txn.open_table(MESSAGES)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── Sessions ──────────────────────────────────────────────────────────────

    pub fn save_session(&self, session: &Session) -> Result<()> {
        let data = serde_json::to_vec(session)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SESSIONS)?;
            table.insert(session.id.as_str(), data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SESSIONS)?;
        match table.get(id)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(SESSIONS)?;
        let mut sessions = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let s: Session = serde_json::from_slice(v.value())?;
            sessions.push(s);
        }
        Ok(sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(SESSIONS)?;
            table.remove(id)?;
        }
        write_txn.commit()?;
        // Also delete messages for this session
        drop(db);
        self.delete_messages_for_session(id)?;
        Ok(())
    }

    // ── Config ────────────────────────────────────────────────────────────────

    pub fn set_config<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let data = serde_json::to_vec(value)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(CONFIG)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_config<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(CONFIG)?;
        match table.get(key)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn delete_config(&self, key: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(CONFIG)?;
            table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── Models ────────────────────────────────────────────────────────────────

    pub fn save_model(&self, model: &ModelConfig) -> Result<()> {
        let data = serde_json::to_vec(model)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MODELS)?;
            table.insert(model.id.as_str(), data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_model(&self, id: &str) -> Result<Option<ModelConfig>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MODELS)?;
        match table.get(id)? {
            Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_models(&self) -> Result<Vec<ModelConfig>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MODELS)?;
        let mut models = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let m: ModelConfig = serde_json::from_slice(v.value())?;
            models.push(m);
        }
        Ok(models)
    }

    pub fn delete_model(&self, id: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MODELS)?;
            table.remove(id)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── MCP Servers ───────────────────────────────────────────────────────────

    pub fn save_mcp_server(&self, server: &McpServer) -> Result<()> {
        let data = serde_json::to_vec(server)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MCP_SERVERS)?;
            table.insert(server.name.as_str(), data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<McpServer>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MCP_SERVERS)?;
        let mut servers = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let s: McpServer = serde_json::from_slice(v.value())?;
            servers.push(s);
        }
        Ok(servers)
    }

    pub fn delete_mcp_server(&self, name: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MCP_SERVERS)?;
            table.remove(name)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── Agent Sessions ────────────────────────────────────────────────────────

    pub fn save_agent_session(&self, agent: &AgentSession) -> Result<()> {
        let data = serde_json::to_vec(agent)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(AGENT_SESSIONS)?;
            table.insert(agent.id.as_str(), data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_agent_sessions(&self) -> Result<Vec<AgentSession>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(AGENT_SESSIONS)?;
        let mut sessions = Vec::new();
        for entry in table.iter()? {
            let (_, v) = entry?;
            let s: AgentSession = serde_json::from_slice(v.value())?;
            sessions.push(s);
        }
        Ok(sessions)
    }

    pub fn delete_agent_session(&self, id: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(AGENT_SESSIONS)?;
            table.remove(id)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // ── Messages ──────────────────────────────────────────────────────────────

    pub fn save_message(&self, msg: &StoredMessage) -> Result<()> {
        let data = serde_json::to_vec(msg)?;
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGES)?;
            table.insert((msg.session_id.as_str(), msg.index), data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(MESSAGES)?;
        let mut messages = Vec::new();
        // Range over all messages for this session
        let start = (session_id, 0u64);
        let end = (session_id, u64::MAX);
        for entry in table.range(start..=end)? {
            let (_, v) = entry?;
            let m: StoredMessage = serde_json::from_slice(v.value())?;
            messages.push(m);
        }
        messages.sort_by_key(|m| m.index);
        Ok(messages)
    }

    pub fn delete_messages_for_session(&self, session_id: &str) -> Result<()> {
        let db = self
            .inner
            .try_lock()
            .map_err(|_| anyhow::anyhow!("DB lock"))?;
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(MESSAGES)?;
            // Collect keys to delete
            let mut keys_to_delete: Vec<u64> = Vec::new();
            {
                let start = (session_id, 0u64);
                let end = (session_id, u64::MAX);
                // Use read access on write transaction table
                for entry in table.range(start..=end)? {
                    let (k, _) = entry?;
                    keys_to_delete.push(k.value().1);
                }
            }
            for idx in keys_to_delete {
                table.remove((session_id, idx))?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn clone_handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
