/// Errors from any [`crate::LlmProvider`].
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Missing API key, bad base URL, unknown provider kind.
    #[error("llm config error: {0}")]
    Config(String),
    /// Network failure before a response arrived.
    #[error("llm transport error: {0}")]
    Transport(String),
    #[error("llm request timed out")]
    Timeout,
    /// Non-2xx response. `message` is the provider's error text (never the API key).
    #[error("llm api error {status}: {message}")]
    Api { status: u16, message: String },
    /// Response body did not match the expected shape.
    #[error("llm response decode error: {0}")]
    Decode(String),
}
