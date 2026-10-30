//! OpenAI Chat Completions provider (`POST {base}/chat/completions`, Bearer auth).
//! Also works with OpenAI-compatible servers via `base_url`.

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::util::{self, non_empty, post_json, schema_or_empty};
use crate::{
    ChatRequest, ChatResponse, LlmError, LlmProvider, Message, ProviderConfig, StopReason,
    ToolCall, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI Chat Completions client.
pub struct OpenAiProvider {
    name: String,
    model: String,
    url: String,
    max_tokens: u32,
    client: reqwest::Client,
}

impl OpenAiProvider {
    /// Build from config, reading the API key from `config.api_key_env`.
    pub fn from_config(config: &ProviderConfig) -> Result<Self, LlmError> {
        let key = util::read_api_key(config)?;
        Self::new(config, &key)
    }

    /// Build with an explicit API key (used by tests and `from_config`).
    pub fn new(config: &ProviderConfig, api_key: &str) -> Result<Self, LlmError> {
        let base = util::resolve_base_url(config, DEFAULT_BASE_URL)?;
        util::require_secure_transport(config, &base)?;
        let mut headers = HeaderMap::new();
        let (_, mut v) =
            util::secret_header("authorization", &format!("Bearer {api_key}"), &config.name)?;
        v.set_sensitive(true);
        headers.insert(AUTHORIZATION, v);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(Self {
            name: config.name.clone(),
            model: config.model.clone(),
            url: format!("{base}/chat/completions"),
            max_tokens: config.max_tokens,
            client: util::build_client(config, headers)?,
        })
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
                        "content": text.as_deref().filter(|t| !t.is_empty()),
                    });
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": c.name,
                                        // OpenAI wants the arguments as a JSON *string*.
                                        "arguments": c.arguments.to_string(),
                                    },
                                })
                            })
                            .collect();
                    }
                    messages.push(msg);
                }
                Message::ToolResult {
                    call_id, content, ..
                } => messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content,
                })),
            }
        }
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            // Newer models reject `max_tokens`.
            "max_completion_tokens": req.max_tokens.unwrap_or(self.max_tokens),
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
        if let Some(temp) = req.temperature {
            body["temperature"] = json!(temp);
        }
        body
    }

    fn parse_response(&self, v: &Value) -> Result<ChatResponse, LlmError> {
        let choice = v
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .ok_or_else(|| LlmError::Decode("openai: response has no `choices`".into()))?;
        let message = choice
            .get("message")
            .ok_or_else(|| LlmError::Decode("openai: choice has no `message`".into()))?;

        let mut text = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if text.is_empty() {
            // A refusal arrives in `refusal` with null content; surface it as text.
            if let Some(r) = message.get("refusal").and_then(Value::as_str) {
                text = r.to_string();
            }
        }

        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| LlmError::Decode("openai: tool call without `id`".into()))?;
                let function = call.get("function").ok_or_else(|| {
                    LlmError::Decode("openai: tool call without `function`".into())
                })?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        LlmError::Decode("openai: tool call without function name".into())
                    })?;
                let arguments = match function.get("arguments") {
                    Some(Value::String(s)) => util::parse_arguments(s, name)?,
                    Some(other) => util::object_arguments(other.clone(), name)?,
                    None => util::parse_arguments("", name)?,
                };
                tool_calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments,
                });
            }
        }

        let stop_reason = match choice.get("finish_reason").and_then(Value::as_str) {
            Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("stop") | None => {
                if tool_calls.is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                }
            }
            Some(other) => StopReason::Other(other.to_string()),
        };

        let usage = v.get("usage").filter(|u| u.is_object()).map(|u| {
            let n = |k: &str| {
                u.get(k)
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32
            };
            Usage {
                input_tokens: n("prompt_tokens"),
                output_tokens: n("completion_tokens"),
            }
        });

        Ok(ChatResponse {
            text: non_empty(text),
            tool_calls,
            stop_reason,
            usage,
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
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
            "openai chat request"
        );
        let v = post_json(&self.client, &self.name, &self.url, &body).await?;
        self.parse_response(&v)
    }
}
