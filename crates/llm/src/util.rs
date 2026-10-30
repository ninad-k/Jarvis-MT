//! Shared plumbing for the provider modules (HTTP client, error mapping, argument parsing).
//! Private to the crate: providers are the public surface.

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};

use crate::{LlmError, ProviderConfig};

/// Resolve the base URL: config override or `default`, trailing slashes removed, validated.
pub(crate) fn resolve_base_url(cfg: &ProviderConfig, default: &str) -> Result<String, LlmError> {
    let raw = cfg
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default);
    let trimmed = raw.trim_end_matches('/').to_string();
    reqwest::Url::parse(&trimmed).map_err(|e| {
        LlmError::Config(format!(
            "provider '{}': invalid base_url '{raw}': {e}",
            cfg.name
        ))
    })?;
    Ok(trimmed)
}

/// Read the API key named by `cfg.api_key_env`. The error names the variable, never the value.
pub(crate) fn read_api_key(cfg: &ProviderConfig) -> Result<String, LlmError> {
    let var = cfg
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            LlmError::Config(format!(
                "provider '{}' ({:?}) requires `api_key_env` naming the environment variable that holds its API key",
                cfg.name, cfg.kind
            ))
        })?;
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(LlmError::Config(format!(
            "environment variable {var} is not set or empty (API key for provider '{}')",
            cfg.name
        ))),
    }
}

/// A header carrying a secret: marked sensitive so reqwest/hyper never print it in debug output.
pub(crate) fn secret_header(
    name: &'static str,
    value: &str,
    provider: &str,
) -> Result<(HeaderName, HeaderValue), LlmError> {
    let mut v = HeaderValue::from_str(value).map_err(|_| {
        LlmError::Config(format!(
            "provider '{provider}': API key contains characters that are not valid in an HTTP header"
        ))
    })?;
    v.set_sensitive(true);
    Ok((HeaderName::from_static(name), v))
}

pub(crate) fn build_client(
    cfg: &ProviderConfig,
    headers: HeaderMap,
) -> Result<reqwest::Client, LlmError> {
    let timeout = Duration::from_secs(cfg.timeout_secs.max(1));
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .default_headers(headers)
        .user_agent(concat!("jarvis-llm/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            LlmError::Config(format!(
                "provider '{}': cannot build HTTP client: {e}",
                cfg.name
            ))
        })
}

pub(crate) fn map_reqwest(e: reqwest::Error) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout
    } else if e.is_decode() {
        LlmError::Decode(e.without_url().to_string())
    } else {
        LlmError::Transport(e.without_url().to_string())
    }
}

/// POST a JSON body, map non-2xx to [`LlmError::Api`], return the parsed JSON body.
/// Request bodies are never logged here.
pub(crate) async fn post_json(
    client: &reqwest::Client,
    provider: &str,
    url: &str,
    body: &Value,
) -> Result<Value, LlmError> {
    let start = Instant::now();
    let resp = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(map_reqwest)?;
    let status = resp.status();
    let text = resp.text().await.map_err(map_reqwest)?;
    tracing::debug!(
        provider,
        status = status.as_u16(),
        elapsed_ms = start.elapsed().as_millis() as u64,
        response_bytes = text.len(),
        "llm http response"
    );
    if !status.is_success() {
        let message = extract_error_message(&text);
        tracing::warn!(provider, status = status.as_u16(), %message, "llm api error");
        return Err(LlmError::Api {
            status: status.as_u16(),
            message,
        });
    }
    serde_json::from_str(&text)
        .map_err(|e| LlmError::Decode(format!("{provider}: response is not valid JSON: {e}")))
}

/// Pull the human-readable message out of a provider error body. Falls back to the
/// (truncated) raw body.
pub(crate) fn extract_error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let candidates = [
            v.pointer("/error/message"),
            v.get("error"),
            v.get("message"),
            v.get("detail"),
        ];
        for c in candidates.into_iter().flatten() {
            if let Some(s) = c.as_str() {
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty response body".to_string();
    }
    truncate(trimmed, 500)
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

/// Parse tool-call arguments sent as a JSON string. Empty -> `{}`; non-object -> Decode.
pub(crate) fn parse_arguments(raw: &str, tool: &str) -> Result<Value, LlmError> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let v: Value = serde_json::from_str(raw).map_err(|e| {
        LlmError::Decode(format!(
            "tool call '{tool}': arguments are not valid JSON: {e}"
        ))
    })?;
    object_arguments(v, tool)
}

/// Normalise already-parsed arguments: null -> `{}`, string -> parsed, object as is.
pub(crate) fn object_arguments(v: Value, tool: &str) -> Result<Value, LlmError> {
    match v {
        Value::Object(_) => Ok(v),
        Value::Null => Ok(Value::Object(Map::new())),
        Value::String(s) => parse_arguments(&s, tool),
        other => Err(LlmError::Decode(format!(
            "tool call '{tool}': arguments must be a JSON object, got {}",
            type_name(&other)
        ))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `Some(text)` unless empty.
pub(crate) fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// A JSON Schema object for tool parameters; falls back to an empty object schema.
pub(crate) fn schema_or_empty(schema: &Value) -> Value {
    match schema {
        Value::Object(m) if !m.is_empty() => schema.clone(),
        _ => serde_json::json!({"type": "object", "properties": {}}),
    }
}
