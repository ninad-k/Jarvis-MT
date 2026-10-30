//! The agent loop: system prompt, per-session history, tool rounds, audit.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use jarvis_llm::{ChatRequest, LlmError, LlmProvider, Message};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use jarvis_gate::intent::Intent;

use crate::audit::{Audit, AuditKind};
use crate::clock::ServerClock;
use crate::tools::{ToolContext, ToolRegistry};
use crate::trading::PreviewDto;

/// Messages kept per session (oldest are dropped first).
pub const MAX_HISTORY_MESSAGES: usize = 40;
/// Model calls per user message. If the last one still wants tools, the loop stops.
pub const MAX_ROUNDS: usize = 5;
/// Sessions kept in memory; the least recently used is evicted beyond this.
const MAX_SESSIONS: usize = 100;

pub const ROUND_LIMIT_REPLY: &str = "Sorry, that took too many steps and I had to stop. \
Could you ask again in a simpler way, or break it into smaller questions?";
const EMPTY_REPLY: &str = "I didn't get an answer from the model. Please try again.";

/// One tool call as reported to the client.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub arguments: Value,
    pub ok: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatOutcome {
    pub session_id: Uuid,
    pub reply: String,
    pub tool_calls: Vec<ToolCallSummary>,
    /// The last preview created during this turn, if any.
    pub preview: Option<PreviewDto>,
}

struct Session {
    messages: Vec<Message>,
    last_used: Instant,
}

pub struct Agent {
    tools: ToolRegistry,
    audit: Audit,
    /// `(spoken alias, broker symbol)` pairs shown to the model.
    aliases: Vec<(String, String)>,
    clock: ServerClock,
    sessions: Mutex<HashMap<Uuid, Session>>,
}

impl Agent {
    pub fn new(tools: ToolRegistry, audit: Audit, clock: ServerClock) -> Self {
        let aliases = tools.resolver().alias_list();
        Self {
            tools,
            audit,
            aliases,
            clock,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Run one user message to completion with `provider`.
    ///
    /// `session_id`: `None` starts a new session; an unknown id starts a fresh session
    /// under that id (history is lost on restart). `voice` marks a spoken turn: previews
    /// created in it get the after-read-back confirm window.
    pub async fn run_turn(
        &self,
        provider: &dyn LlmProvider,
        session_id: Option<Uuid>,
        message: &str,
        voice: bool,
    ) -> Result<ChatOutcome, LlmError> {
        let sid = session_id.unwrap_or_else(Uuid::new_v4);
        let mut history = self.snapshot(sid);
        let turn_start = history.len();
        history.push(Message::User {
            text: message.to_owned(),
        });
        self.audit
            .record(
                Some(sid),
                AuditKind::Utterance,
                json!({ "text": message, "provider": provider.name(), "model": provider.model() }),
            )
            .await;

        // What the user actually said, as far as the deterministic parser can tell. Every
        // propose_* call is cross-checked against it (SPEC rule 9).
        let intent = self.tools.parse_intent(message);
        if intent != Intent::Other {
            self.audit
                .record(
                    Some(sid),
                    AuditKind::Intent,
                    json!({ "text": message, "intent": &intent, "voice": voice }),
                )
                .await;
        }
        let ctx = ToolContext {
            session_id: sid,
            intent,
            voice,
        };

        let system = self.system_prompt();
        let tools = self.tools.specs();
        let mut summaries = Vec::new();
        let mut preview: Option<PreviewDto> = None;
        let reply;

        let mut round = 0;
        loop {
            round += 1;
            let request = ChatRequest {
                system: Some(system.clone()),
                messages: history.clone(),
                tools: tools.clone(),
                max_tokens: None,
                temperature: Some(0.2),
            };
            let response = match provider.chat(&request).await {
                Ok(r) => r,
                Err(e) => {
                    self.audit
                        .record(
                            Some(sid),
                            AuditKind::Error,
                            json!({ "where": "llm", "provider": provider.name(), "message": e.to_string() }),
                        )
                        .await;
                    return Err(e);
                }
            };

            if response.tool_calls.is_empty() {
                let text = response
                    .text
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty());
                let text = text.unwrap_or_else(|| EMPTY_REPLY.to_owned());
                history.push(Message::Assistant {
                    text: Some(text.clone()),
                    tool_calls: vec![],
                });
                reply = text;
                break;
            }

            if round >= MAX_ROUNDS {
                // Keep history valid: no dangling tool calls.
                tracing::warn!(session = %sid, "agent stopped after {MAX_ROUNDS} model rounds");
                history.push(Message::Assistant {
                    text: Some(ROUND_LIMIT_REPLY.to_owned()),
                    tool_calls: vec![],
                });
                reply = ROUND_LIMIT_REPLY.to_owned();
                break;
            }

            history.push(Message::Assistant {
                text: response.text.clone(),
                tool_calls: response.tool_calls.clone(),
            });
            for call in &response.tool_calls {
                let started = Instant::now();
                let outcome = self
                    .tools
                    .execute_in(&ctx, &call.name, &call.arguments)
                    .await;
                if let Some(p) = &outcome.preview {
                    preview = Some(p.clone());
                }
                let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

                if let Some(req) = &outcome.bridge_request {
                    self.audit
                        .record(Some(sid), AuditKind::BridgeRequest, req.clone())
                        .await;
                }
                if let Some(reply) = &outcome.bridge_reply {
                    self.audit
                        .record(Some(sid), AuditKind::BridgeReply, reply.clone())
                        .await;
                }
                self.audit
                    .record(
                        Some(sid),
                        AuditKind::ToolCall,
                        json!({
                            "name": call.name,
                            "arguments": call.arguments,
                            "ok": !outcome.is_error,
                            "duration_ms": duration_ms,
                            "result": truncate(&outcome.content, 2000),
                        }),
                    )
                    .await;

                summaries.push(ToolCallSummary {
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    ok: !outcome.is_error,
                    duration_ms,
                });
                history.push(Message::ToolResult {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: outcome.content,
                    is_error: outcome.is_error,
                });
            }
        }

        self.audit
            .record(
                Some(sid),
                AuditKind::AssistantReply,
                json!({ "text": reply, "provider": provider.name(), "tool_calls": summaries.len() }),
            )
            .await;
        self.append(sid, history.split_off(turn_start));
        Ok(ChatOutcome {
            session_id: sid,
            reply,
            tool_calls: summaries,
            preview,
        })
    }

    fn snapshot(&self, sid: Uuid) -> Vec<Message> {
        let mut sessions = self.lock();
        sessions
            .get_mut(&sid)
            .map(|s| {
                s.last_used = Instant::now();
                s.messages.clone()
            })
            .unwrap_or_default()
    }

    /// Append this turn's messages to the stored session, then enforce the cap.
    fn append(&self, sid: Uuid, new_messages: Vec<Message>) {
        let mut sessions = self.lock();
        if !sessions.contains_key(&sid)
            && sessions.len() >= MAX_SESSIONS
            && let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(id, _)| *id)
        {
            sessions.remove(&oldest);
        }
        let session = sessions.entry(sid).or_insert_with(|| Session {
            messages: Vec::new(),
            last_used: Instant::now(),
        });
        session.messages.extend(new_messages);
        session.last_used = Instant::now();
        trim_history(&mut session.messages);
    }

    /// Number of messages stored for a session (tests, diagnostics).
    pub fn history_len(&self, sid: Uuid) -> usize {
        self.lock().get(&sid).map_or(0, |s| s.messages.len())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Session>> {
        // A poisoned lock only means another request panicked; the data is still usable.
        self.sessions.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn system_prompt(&self) -> String {
        build_system_prompt(
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            self.clock
                .now_server()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            &self.aliases,
        )
    }
}

/// Keep at most [`MAX_HISTORY_MESSAGES`], and never start mid-exchange (a tool result or
/// assistant tool call without its user message confuses providers).
fn trim_history(messages: &mut Vec<Message>) {
    if messages.len() > MAX_HISTORY_MESSAGES {
        let excess = messages.len() - MAX_HISTORY_MESSAGES;
        messages.drain(..excess);
    }
    let first_user = messages
        .iter()
        .position(|m| matches!(m, Message::User { .. }));
    match first_user {
        Some(0) => {}
        Some(i) => {
            messages.drain(..i);
        }
        None => messages.clear(),
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max_chars).collect();
        format!("{cut}...[truncated]")
    }
}

fn build_system_prompt(
    now_utc: String,
    server_time: String,
    aliases: &[(String, String)],
) -> String {
    let alias_text = if aliases.is_empty() {
        "none configured".to_owned()
    } else {
        aliases
            .iter()
            .map(|(a, s)| format!("{a} = {s}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "You are Jarvis, a trading assistant for a single trader using MetaTrader 5 (MT5).
         Current time: {now_utc} UTC. MT5 broker server time: {server_time} (deal times and            history dates use server time).
         
         What you can do:
         - Read live data with your tools: prices (get_price), the account summary            (get_account), open positions (get_positions), pending orders (get_orders) and            trade history (get_history).
         - Propose trades with propose_order, propose_modify, propose_close and propose_cancel.            PROPOSING DOES NOT EXECUTE ANYTHING. A proposal only creates a preview. The user must            confirm it in the app (the Confirm button, or by saying \"confirm\") within a few            seconds. You cannot confirm, cancel or execute anything yourself.
         - Never state a price, balance, profit, position or any other number that you did not            get from a tool in this conversation. Prices move: for any question about a current            value, call the tool again instead of reusing an earlier answer. If a tool fails, say            so plainly; do not guess.
         
         Trading rules (strict):
         - After a propose tool succeeds, tell the user the preview in one or two sentences (use            the tool's description and numbers) and ask them to confirm. Say that nothing has            happened yet. If the result says two confirmations are needed, say so.
         - NEVER say or imply that a trade was placed, modified, closed or cancelled unless the            user tells you so or a tool result explicitly says it happened. A propose tool result            means \"awaiting confirmation\", never \"done\". If the user asks whether it went            through, check with get_positions or get_orders.
         - If the side (buy or sell), the symbol or the volume is missing or unclear, ask one            short clarifying question. Never invent a volume or a symbol, and never assume a            default lot size. Use exactly the numbers the user said for volume, price, stop loss            and take profit.
         - Propose only what the user asked for in their latest message. A new proposal replaces            the pending one.
         - If a propose tool is refused (a limit such as maximum lot, maximum positions, daily            loss, allowed symbols or trading hours; the kill switch; a mismatch with what the            user said), explain briefly why, name the limit and its value, and do not retry with            different numbers unless the user asks for that.
         - If the user says confirm, yes, cancel or no about a pending preview, do not propose            again: the app handles it. Remind them to press Confirm or Cancel, or say \"confirm\"            or \"cancel\".
         
         How to behave:
         - If a request is ambiguous (which symbol? which position? which dates?), ask one short            clarifying question instead of guessing.
         - Symbol aliases (the user's words -> broker symbol): {alias_text}. You may pass the            user's own word to a tool; it resolves aliases itself. Speech-to-text can mishear            names (for example \"cold\" for \"gold\").
         - Be concise and easy to read aloud: 1 to 3 sentences unless you are listing items.            No markdown tables, no emojis. Say numbers plainly with their symbol or currency.
         - You are not a financial adviser; do not give investment recommendations."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> Message {
        Message::User { text: t.into() }
    }
    fn asst(t: &str) -> Message {
        Message::Assistant {
            text: Some(t.into()),
            tool_calls: vec![],
        }
    }

    #[test]
    fn trim_keeps_cap_and_starts_on_user_message() {
        let mut msgs = Vec::new();
        for i in 0..30 {
            msgs.push(user(&format!("q{i}")));
            msgs.push(asst(&format!("a{i}")));
        }
        trim_history(&mut msgs);
        assert!(msgs.len() <= MAX_HISTORY_MESSAGES);
        assert!(matches!(msgs[0], Message::User { .. }));
        assert_eq!(msgs.last(), Some(&asst("a29")));
    }

    #[test]
    fn trim_drops_orphan_leading_tool_messages() {
        let mut msgs = vec![
            Message::ToolResult {
                call_id: "1".into(),
                name: "get_price".into(),
                content: "{}".into(),
                is_error: false,
            },
            asst("x"),
            user("next"),
            asst("y"),
        ];
        trim_history(&mut msgs);
        assert_eq!(msgs, vec![user("next"), asst("y")]);
    }

    #[test]
    fn prompt_covers_the_core_rules() {
        let p = build_system_prompt(
            "2026-10-30T10:00:00Z".into(),
            "2026-10-30 13:00:00".into(),
            &[
                ("gold".into(), "XAUUSD".into()),
                ("dow jones".into(), "US30".into()),
            ],
        );
        assert!(p.contains("2026-10-30T10:00:00Z"));
        assert!(p.contains("2026-10-30 13:00:00"));
        assert!(p.contains("gold = XAUUSD"));
        assert!(p.contains("dow jones = US30"));
        assert!(p.contains("propose_order"));
        assert!(p.contains("PROPOSING DOES NOT EXECUTE ANYTHING"));
        assert!(p.contains("NEVER say or imply that a trade was placed"));
        assert!(p.contains("Never invent a volume"));
        assert!(p.contains("name the limit"));
        assert!(p.contains("clarifying question"));
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert!(truncate(&"é".repeat(50), 10).starts_with("éééééééééé...["));
    }
}
