use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use super::{LlmProvider, LlmResponse, Message, Tool, ToolCall};

pub struct ClaudeCodeProvider {
    model: String,
    client: Client,
}

impl ClaudeCodeProvider {
    pub fn new(model: String) -> Self {
        Self {
            model,
            client: Client::new(),
        }
    }

    fn credentials_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".claude").join(".credentials.json")
    }

    pub fn is_available() -> bool {
        let path = Self::credentials_path();
        if !path.exists() {
            return false;
        }
        read_access_token().is_ok()
    }
}

fn read_access_token() -> Result<String> {
    let path = ClaudeCodeProvider::credentials_path();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .context("Failed to parse Claude Code credentials")?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No accessToken in Claude Code credentials"))
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

#[async_trait]
impl LlmProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "Claude Code"
    }

    async fn complete(&self, messages: &[Message], tools: &[Tool], system_prompt: Option<&str>) -> Result<LlmResponse> {
        let token = read_access_token()?;

        let anthropic_messages: Vec<AnthropicMessage> = messages
            .iter()
            .map(|m| AnthropicMessage {
                role: m.role.clone(),
                content: json!(m.content),
            })
            .collect();

        let anthropic_tools: Vec<AnthropicTool> = tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        let request = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 4096,
            messages: anthropic_messages,
            tools: anthropic_tools,
            system: system_prompt.map(|s| s.to_string()),
        };

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("Authorization", format!("Bearer {}", token))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Anthropic")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(anyhow::anyhow!("Anthropic API error {}: {}", status, body));
        }

        let anthropic_resp: AnthropicResponse = response
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for content in anthropic_resp.content {
            match content.content_type.as_str() {
                "text" => {
                    if let Some(text) = content.text {
                        text_parts.push(text);
                    }
                }
                "tool_use" => {
                    if let (Some(id), Some(name), Some(input)) =
                        (content.id, content.name, content.input)
                    {
                        tool_calls.push(ToolCall { id, name, input });
                    }
                }
                _ => {}
            }
        }

        Ok(LlmResponse {
            content: text_parts.join("\n"),
            tool_calls,
        })
    }
}
