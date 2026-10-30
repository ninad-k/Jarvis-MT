//! Provider-agnostic chat with tool calling.
//!
//! [`LlmProvider`] is the only thing the rest of the backend sees. Implementations:
//! Anthropic Messages API, OpenAI Chat Completions, Ollama `/api/chat`.
//! Build one from config with [`build_provider`].

mod error;
mod types;
mod util;

pub mod anthropic;
pub mod ollama;
pub mod openai;

pub use error::LlmError;
pub use types::*;

use async_trait::async_trait;

/// A chat model that can call tools.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Config name of this provider instance, e.g. `"local"` or `"claude"`.
    fn name(&self) -> &str;
    /// Model id sent to the API, e.g. `"qwen3.5:9b"`.
    fn model(&self) -> &str;
    /// One model turn. The caller runs the tool loop: when the response has
    /// `tool_calls`, it executes them, appends `Message::Assistant` + `Message::ToolResult`s
    /// and calls `chat` again.
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError>;
}

/// Build a provider from config. Reads the API key from the env var named in
/// `config.api_key_env` (required for Anthropic/OpenAI, ignored for Ollama).
pub fn build_provider(config: &ProviderConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    Ok(match config.kind {
        ProviderKind::Anthropic => Box::new(anthropic::AnthropicProvider::from_config(config)?),
        ProviderKind::Openai => Box::new(openai::OpenAiProvider::from_config(config)?),
        ProviderKind::Ollama => Box::new(ollama::OllamaProvider::from_config(config)?),
    })
}
