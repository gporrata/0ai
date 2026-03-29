pub mod anthropic;
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
    async fn complete(&self, messages: &[Message], tools: &[Tool]) -> Result<LlmResponse>;
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
            id: "claude-opus-4-5".to_string(),
            provider: "anthropic".to_string(),
            display_name: "Anthropic claude-opus-4-5".to_string(),
            configured: false,
        },
        ModelInfo {
            id: "claude-sonnet-4-6".to_string(),
            provider: "anthropic".to_string(),
            display_name: "Anthropic claude-sonnet-4-6".to_string(),
            configured: false,
        },
        ModelInfo {
            id: "claude-haiku-4-5".to_string(),
            provider: "anthropic".to_string(),
            display_name: "Anthropic claude-haiku-4-5".to_string(),
            configured: false,
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
