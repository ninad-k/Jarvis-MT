//! Ollama provider (`POST {base}/api/chat`, non-streaming).

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::util::{self, non_empty, post_json, schema_or_empty};
use crate::{
    ChatRequest, ChatResponse, LlmError, LlmProvider, Message, ProviderConfig, StopReason,
    ToolCall, Usage,
};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Ollama `/api/chat` client. No API key.
pub struct OllamaProvider {
    name: String,
    model: String,
    url: String,
    max_tokens: u32,
    client: reqwest::Client,
    think: bool,
}

impl OllamaProvider {
    /// Build from config (the API key, if any, is ignored).
    pub fn from_config(config: &ProviderConfig) -> Result<Self, LlmError> {
        Self::new(config)
    }

    pub fn new(config: &ProviderConfig) -> Result<Self, LlmError> {
        let base = util::resolve_base_url(config, DEFAULT_BASE_URL)?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(Self {
            name: config.name.clone(),
            model: config.model.clone(),
            url: format!("{base}/api/chat"),
            max_tokens: config.max_tokens,
            client: util::build_client(config, headers)?,
            think: false,
        })
    }

    /// Enable or disable the model's "thinking" phase (sent as `think`). Default `false`:
    /// thinking-capable models (Qwen3 family) are much slower with it on.
    pub fn with_think(mut self, think: bool) -> Self {
        self.think = think;
        self
    }

    fn build_body(&self, req: &ChatRequest) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(system) = req.system.as_deref().filter(|s| !s.trim().is_empty()) {
            messages.push(json!({"role": "system", "content": system}));
        }
        for m in &req.messages {
            match m {
                Message::User { text } => messages.push(json!({"role": "user", "content": text})),
                Message::Assistant { text, tool_calls } => {
                    let mut msg = json!({
                        "role": "assistant",
                        "content": text.as_deref().unwrap_or_default(),
                    });
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = tool_calls
                            .iter()
                            .map(
                                |c| json!({"function": {"name": c.name, "arguments": c.arguments}}),
                            )
                            .collect();
                    }
                    messages.push(msg);
                }
                Message::ToolResult { name, content, .. } => messages.push(json!({
                    "role": "tool",
                    "tool_name": name,
                    "content": content,
                })),
            }
        }
        let mut options = json!({"num_predict": req.max_tokens.unwrap_or(self.max_tokens)});
        if let Some(t) = req.temperature {
            options["temperature"] = json!(t);
        }
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "think": self.think,
            "options": options,
        });
        if !req.tools.is_empty() {
            body["tools"] = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": schema_or_empty(&t.parameters),
                        },
                    })
                })
                .collect();
        }
        body
    }

    fn parse_response(&self, v: &Value) -> Result<ChatResponse, LlmError> {
        let message = v
            .get("message")
            .filter(|m| m.is_object())
            .ok_or_else(|| LlmError::Decode("ollama: response has no `message`".into()))?;
        let text = strip_think_tags(
            message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );

        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (i, call) in calls.iter().enumerate() {
                let function = call.get("function").ok_or_else(|| {
                    LlmError::Decode("ollama: tool call without `function`".into())
                })?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        LlmError::Decode("ollama: tool call without function name".into())
                    })?;
                let arguments = util::object_arguments(
                    function.get("arguments").cloned().unwrap_or(Value::Null),
                    name,
                )?;
                tool_calls.push(ToolCall {
                    // Ollama sends no call ids; index-based ids are stable within a response.
                    id: format!("call_{i}"),
                    name: name.to_string(),
                    arguments,
                });
            }
        }

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            match v.get("done_reason").and_then(Value::as_str) {
                Some("length") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            }
        };

        let usage = match (v.get("prompt_eval_count"), v.get("eval_count")) {
            (None, None) => None,
            (p, e) => {
                let n = |x: Option<&Value>| {
                    x.and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32
                };
                Some(Usage {
                    input_tokens: n(p),
                    output_tokens: n(e),
                })
            }
        };

        Ok(ChatResponse {
            text: non_empty(text),
            tool_calls,
            stop_reason,
            usage,
        })
    }
}

/// Some models (older Qwen3 tags) emit their reasoning inline as `<think>...</think>` even
/// with `think: false`. Drop it so the user (and TTS) only see the answer.
fn strip_think_tags(content: &str) -> String {
    let mut s = content;
    if let Some(i) = s.rfind("</think>") {
        s = &s[i + "</think>".len()..];
    } else if let Some(i) = s.find("<think>") {
        // Unterminated reasoning (e.g. cut off by the token limit): keep only what preceded it.
        s = &s[..i];
    }
    s.trim().to_string()
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = self.build_body(request);
        tracing::debug!(
            provider = %self.name,
            model = %self.model,
            messages = request.messages.len(),
            tools = request.tools.len(),
            think = self.think,
            "ollama chat request"
        );
        let v = post_json(&self.client, &self.name, &self.url, &body).await?;
        self.parse_response(&v)
    }
}
