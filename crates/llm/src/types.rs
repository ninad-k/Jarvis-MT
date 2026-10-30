use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which API a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Ollama,
}

/// One `[[llm.providers]]` entry from `config/jarvis.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Name used to select this provider, e.g. `"local"`, `"claude"`.
    pub name: String,
    pub kind: ProviderKind,
    pub model: String,
    /// Override the API base URL. Defaults: `https://api.anthropic.com`,
    /// `https://api.openai.com/v1`, `http://127.0.0.1:11434`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Name of the env var holding the API key, e.g. `"ANTHROPIC_API_KEY"`.
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_timeout_secs() -> u64 {
    60
}
fn default_max_tokens() -> u32 {
    1024
}

/// A tool the model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider call id (Ollama has none: the provider generates one).
    pub id: String,
    pub name: String,
    /// Parsed JSON arguments (always an object; `{}` when the model sent none).
    pub arguments: Value,
}

/// Conversation history in a provider-neutral shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
    },
    /// Result of executing a tool call. `content` is usually JSON text.
    ToolResult {
        call_id: String,
        name: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// Overrides `ProviderConfig::max_tokens` when set.
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Assistant text, if any (may be present alongside tool calls).
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Option<Usage>,
}
