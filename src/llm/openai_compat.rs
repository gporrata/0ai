use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{LlmProvider, LlmResponse, Message, Tool, ToolCall};

pub struct OpenAiCompatProvider {
    api_key: String,
    model: String,
    endpoint: String,
    provider_name: String,
    client: Client,
}

impl OpenAiCompatProvider {
    pub fn new(api_key: String, model: String, endpoint: String, provider_name: String) -> Self {
        Self {
            api_key,
            model,
            endpoint,
            provider_name,
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    max_tokens: u32,
}

#[derive(Serialize)]
struct OaiMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiFunction,
}

#[derive(Serialize)]
struct OaiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
}

#[derive(Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
}

#[derive(Deserialize)]
struct OaiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OaiToolCall>>,
}

#[derive(Deserialize)]
struct OaiToolCall {
    id: String,
    function: OaiToolCallFunction,
}

#[derive(Deserialize)]
struct OaiToolCallFunction {
    name: String,
    arguments: String,
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn complete(&self, messages: &[Message], tools: &[Tool]) -> Result<LlmResponse> {
        let oai_messages: Vec<OaiMessage> = messages
            .iter()
            .map(|m| OaiMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let oai_tools: Vec<OaiTool> = tools
            .iter()
            .map(|t| OaiTool {
                tool_type: "function".to_string(),
                function: OaiFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect();

        let request = OaiRequest {
            model: self.model.clone(),
            messages: oai_messages,
            tools: oai_tools,
            max_tokens: 4096,
        };

        let url = format!("{}/chat/completions", self.endpoint.trim_end_matches('/'));

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context("Failed to send request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(anyhow::anyhow!("API error {}: {}", status, body));
        }

        let oai_resp: OaiResponse = response.json().await.context("Failed to parse response")?;

        let choice = oai_resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No choices in response"))?;

        let content = choice.message.content.unwrap_or_default();

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let input = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    input,
                }
            })
            .collect();

        Ok(LlmResponse {
            content,
            tool_calls,
        })
    }
}
