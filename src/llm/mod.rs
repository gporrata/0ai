pub mod anthropic;
pub mod claude_code;
pub mod ollama;
pub mod openai_compat;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        system_prompt: Option<&str>,
    ) -> Result<LlmResponse>;
}

/// Gather OS/environment context for the system prompt.
pub fn build_system_prompt() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let hostname = std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string();
    let hostname = if hostname.is_empty() { "unknown".to_string() } else { hostname };

    // Linux distro from /etc/os-release
    let distro = std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
        .unwrap_or_else(|| os.to_string());

    format!(
        "You are a helpful AI assistant running on the user's local machine.\n\
         System information:\n\
         - OS: {distro}\n\
         - Kernel/platform: {os} {arch}\n\
         - Shell: {shell}\n\
         - User: {username}@{hostname}\n\
         When providing commands or file paths, use syntax appropriate for this system."
    )
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub configured: bool,
}

pub fn all_known_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "claude-opus-4-6".to_string(),
            provider: "claude_code".to_string(),
            display_name: "Claude Code claude-opus-4-6".to_string(),
            configured: claude_code::ClaudeCodeProvider::is_available(),
        },
        ModelInfo {
            id: "claude-sonnet-4-6".to_string(),
            provider: "claude_code".to_string(),
            display_name: "Claude Code claude-sonnet-4-6".to_string(),
            configured: claude_code::ClaudeCodeProvider::is_available(),
        },
        ModelInfo {
            id: "claude-haiku-4-5".to_string(),
            provider: "claude_code".to_string(),
            display_name: "Claude Code claude-haiku-4-5".to_string(),
            configured: claude_code::ClaudeCodeProvider::is_available(),
        },
        ModelInfo {
            id: "grok-4".to_string(),
            provider: "xai".to_string(),
            display_name: "xAI grok-4".to_string(),
            configured: false,
        },
        ModelInfo {
            id: "grok-3".to_string(),
            provider: "xai".to_string(),
            display_name: "xAI grok-3".to_string(),
            configured: false,
        },
        ModelInfo {
            id: "grok-3-mini".to_string(),
            provider: "xai".to_string(),
            display_name: "xAI grok-3-mini".to_string(),
            configured: false,
        },
        ModelInfo {
            id: "llama3.2".to_string(),
            provider: "ollama".to_string(),
            display_name: "Ollama llama3.2 (local)".to_string(),
            configured: true, // ollama needs no API key
        },
        ModelInfo {
            id: "mistral".to_string(),
            provider: "ollama".to_string(),
            display_name: "Ollama mistral (local)".to_string(),
            configured: true,
        },
        ModelInfo {
            id: "phi4".to_string(),
            provider: "ollama".to_string(),
            display_name: "Ollama phi4 (local)".to_string(),
            configured: true,
        },
    ]
}

pub fn build_provider(
    model_id: &str,
    provider: &str,
    api_key: Option<&str>,
    endpoint: Option<&str>,
) -> Result<Box<dyn LlmProvider>> {
    match provider {
        "claude_code" => Ok(Box::new(claude_code::ClaudeCodeProvider::new(
            model_id.to_string(),
        ))),
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new(
            api_key.unwrap_or("").to_string(),
            model_id.to_string(),
        ))),
        "xai" => Ok(Box::new(openai_compat::OpenAiCompatProvider::new(
            api_key.unwrap_or("").to_string(),
            model_id.to_string(),
            endpoint
                .unwrap_or("https://api.x.ai/v1")
                .to_string(),
            "xAI".to_string(),
        ))),
        "ollama" => Ok(Box::new(ollama::OllamaProvider::new(
            model_id.to_string(),
            endpoint
                .unwrap_or("http://localhost:11434")
                .to_string(),
        ))),
        _ => Err(anyhow::anyhow!("Unknown provider: {}", provider)),
    }
}
