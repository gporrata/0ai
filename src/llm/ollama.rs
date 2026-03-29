use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{LlmProvider, LlmResponse, Message, Tool};

pub struct OllamaProvider {
    model: String,
    endpoint: String,
    client: Client,
}

impl OllamaProvider {
    pub fn new(model: String, endpoint: String) -> Self {
        Self {
            model,
            endpoint,
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaResponseMessage,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "Ollama"
    }

    async fn complete(&self, messages: &[Message], _tools: &[Tool]) -> Result<LlmResponse> {
        let ollama_messages: Vec<OllamaMessage> = messages
            .iter()
            .map(|m| OllamaMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let request = OllamaRequest {
            model: self.model.clone(),
            messages: ollama_messages,
            stream: false,
        };

        let url = format!(
            "{}/api/chat",
            self.endpoint.trim_end_matches('/')
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to connect to Ollama (is it running?)")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(anyhow::anyhow!("Ollama error {}: {}", status, body));
        }

        let ollama_resp: OllamaResponse =
            response.json().await.context("Failed to parse Ollama response")?;

        Ok(LlmResponse {
            content: ollama_resp.message.content,
            tool_calls: Vec::new(),
        })
    }
}
