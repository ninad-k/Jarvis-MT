//! Voice: speech-to-text and text-to-speech routes over `jarvis-voice`.
//!
//! The engines are built once at startup. Any failure there (missing model, Piper not
//! found, whisper not compiled in) is logged as a warning and leaves that half of voice
//! switched off; it never stops the server.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jarvis_gate::intent::{self, Intent};
use jarvis_persona::PersonaPack;
use jarvis_voice::{
    Corrections, PiperTts, SpeechToText, TextToSpeech, VoiceConfig, VoiceError, WhisperStt,
    decode_wav, default_corrections, normalize_transcript,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::audit::AuditKind;
use crate::auth::http::MaybeSession;
use crate::events::Event;
use crate::routes::ApiError;
use crate::state::AppState;

/// Longest text `POST /api/voice/speak` accepts, in characters.
pub const MAX_SPEAK_CHARS: usize = 600;
/// How long a transcription request waits for the one running before it gives up.
const STT_QUEUE_WAIT: Duration = Duration::from_secs(5);
/// Text-to-speech runs a child process per request; allow two at once.
const TTS_CONCURRENCY: usize = 2;

/// The voice engines and the gates that protect them.
pub struct Voice {
    stt: Option<Arc<dyn SpeechToText>>,
    /// The voice of `[voice] piper_voice`: used for a persona that has no voice of its own.
    tts: Option<Arc<dyn TextToSpeech>>,
    /// One engine per persona whose `voice_model` exists.
    persona_tts: HashMap<String, Arc<dyn TextToSpeech>>,
    max_seconds: f32,
    corrections: Corrections,
    /// One transcription at a time: protects the GPU/CPU.
    stt_gate: Semaphore,
    tts_gate: Semaphore,
}

impl Voice {
    /// Voice switched off.
    pub fn disabled() -> Self {
        Self::new(None, None, 30.0)
    }

    pub fn new(
        stt: Option<Arc<dyn SpeechToText>>,
        tts: Option<Arc<dyn TextToSpeech>>,
        max_seconds: f32,
    ) -> Self {
        Self {
            stt,
            tts,
            persona_tts: HashMap::new(),
            max_seconds,
            corrections: default_corrections(),
            stt_gate: Semaphore::new(1),
            tts_gate: Semaphore::new(TTS_CONCURRENCY),
        }
    }

    /// Give a persona its own voice.
    pub fn with_persona_tts(mut self, persona: &str, tts: Arc<dyn TextToSpeech>) -> Self {
        self.persona_tts.insert(persona.to_owned(), tts);
        self
    }

    /// Build the engines from `[voice]` and one Piper voice per persona whose `voice_model`
    /// file exists. Never fails: a broken half is left off.
    pub async fn from_config(config: &VoiceConfig, personas: &[PersonaPack]) -> Self {
        let stt_config = config.clone();
        let stt = tokio::task::spawn_blocking(move || {
            WhisperStt::new(&stt_config, default_corrections())
        })
        .await;
        let stt: Option<Arc<dyn SpeechToText>> = match stt {
            Ok(Ok(engine)) => Some(Arc::new(engine)),
            Ok(Err(e)) => {
                tracing::warn!("speech-to-text is off: {e}");
                None
            }
            Err(e) => {
                tracing::warn!("speech-to-text is off: loading the model failed: {e}");
                None
            }
        };
        let tts: Option<Arc<dyn TextToSpeech>> = match PiperTts::new(config) {
            Ok(engine) => Some(Arc::new(engine)),
            Err(e) => {
                tracing::warn!("text-to-speech is off: {e}");
                None
            }
        };
        let mut voice = Self::new(stt, tts, config.max_seconds);
        for persona in personas {
            if !persona.voice_model.is_file() {
                tracing::warn!(
                    persona = %persona.id,
                    "voice model {} not found; this persona has no voice",
                    persona.voice_model.display()
                );
                continue;
            }
            let own = VoiceConfig {
                piper_voice: persona.voice_model.clone(),
                ..config.clone()
            };
            match PiperTts::new(&own) {
                Ok(engine) => {
                    voice
                        .persona_tts
                        .insert(persona.id.clone(), Arc::new(engine));
                }
                Err(e) => tracing::warn!(persona = %persona.id, "no voice for this persona: {e}"),
            }
        }
        voice
    }

    pub fn stt_ready(&self) -> bool {
        self.stt.is_some()
    }
    pub fn tts_ready(&self) -> bool {
        self.tts.is_some() || !self.persona_tts.is_empty()
    }
    /// True when the persona has a voice of its own (its model file exists and loaded).
    pub fn persona_voice_available(&self, persona: &str) -> bool {
        self.persona_tts.contains_key(persona)
    }
    /// The engine that speaks for `persona`: its own voice, else the shared one.
    fn tts_for(&self, persona: &str) -> Option<Arc<dyn TextToSpeech>> {
        self.persona_tts
            .get(persona)
            .cloned()
            .or_else(|| self.tts.clone())
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

fn unavailable(what: &str) -> ApiError {
    ApiError::VoiceUnavailable(format!(
        "{what} is not available on this server (see [voice] in the config and the server log)"
    ))
}

fn is_wav(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|t| matches!(t.as_str(), "audio/wav" | "audio/x-wav" | "audio/wave"))
}

#[derive(Deserialize)]
pub struct TranscribeQuery {
    /// `wake=1` (or `true`): look for a persona's wake phrase at the start of the speech.
    #[serde(default)]
    wake: Option<String>,
}

fn flag(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// `POST /api/voice/transcribe[?wake=1]`
pub async fn transcribe(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    Query(query): Query<TranscribeQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let voice = state.voice();
    let stt = voice
        .stt
        .clone()
        .ok_or_else(|| unavailable("speech-to-text"))?;
    if !is_wav(&headers) {
        return Err(ApiError::BadRequest(
            "send the recording as audio/wav (16-bit PCM)".to_owned(),
        ));
    }
    if body.is_empty() {
        return Err(ApiError::BadRequest("the recording is empty".to_owned()));
    }

    let max_seconds = voice.max_seconds;
    let samples = tokio::task::spawn_blocking(move || decode_wav(&body, max_seconds))
        .await
        .map_err(|e| {
            tracing::error!("decoding task failed: {e}");
            ApiError::Internal
        })?
        .map_err(|e| match e {
            VoiceError::Audio(m) => ApiError::BadRequest(format!("unsupported audio: {m}")),
            other => {
                tracing::error!("decoding audio failed: {other}");
                ApiError::Internal
            }
        })?;

    let _permit = match tokio::time::timeout(STT_QUEUE_WAIT, voice.stt_gate.acquire()).await {
        Ok(Ok(permit)) => permit,
        _ => {
            return Err(ApiError::RateLimited {
                retry_after_secs: 2,
            });
        }
    };
    let transcript = stt.transcribe(samples).await.map_err(|e| {
        tracing::error!("speech-to-text failed: {e}");
        ApiError::Internal
    })?;
    drop(_permit);

    let text = normalize_transcript(&transcript.text, &voice.corrections);

    // Wake-word mode: a persona's wake phrase at the start picks that persona for the user and
    // is cut off; the rest is the command. Nothing else about the speech changes.
    let wake_mode = flag(query.wake.as_deref());
    let mut settings = state.user_settings(session.as_ref()).await;
    let mut wake = None;
    if wake_mode && let Some((pack, command)) = state.personas().match_wake_word(&text) {
        // Never change persona while a confirmation is open: the flow must finish with the
        // face it started with. The phrase is still detected and its command still used.
        if pack.id != settings.persona && !state.trading().has_pending_confirmation() {
            match state.set_user_persona(session.as_ref(), &pack.id).await {
                Ok(updated) => {
                    settings = updated;
                    state.events().emit(Event::SettingsChanged);
                }
                Err(e) => {
                    tracing::error!("saving the persona chosen by the wake phrase failed: {e:#}");
                    return Err(ApiError::Internal);
                }
            }
        }
        wake = Some((pack.id.clone(), command));
    }

    let command_text = wake.as_ref().map_or(text.as_str(), |(_, c)| c.as_str());
    let resolver = state.resolver_for(&settings);
    let aliases = resolver.alias_list().into_iter().collect();
    let intent = match intent::parse(command_text, &aliases) {
        Intent::Confirm => "confirm",
        Intent::Cancel => "cancel",
        _ => "other",
    };
    let wake_json = wake_mode.then(|| match &wake {
        Some((persona, command)) => json!({
            "detected": true,
            "persona": persona,
            "command": command,
        }),
        None => json!({ "detected": false, "persona": null, "command": "" }),
    });
    // The text and its timing only; the audio is never stored.
    let mut audit = json!({
        "text": text,
        "raw": transcript.raw,
        "audio_ms": transcript.audio_ms,
        "elapsed_ms": transcript.elapsed_ms,
        "intent": intent,
    });
    let mut reply = audit.clone();
    if let (Some(w), Some(a), Some(r)) = (wake_json, audit.as_object_mut(), reply.as_object_mut()) {
        a.insert("wake".to_owned(), w.clone());
        r.insert("wake".to_owned(), w);
    }
    state
        .audit()
        .record_as(None, Some(&settings.persona), AuditKind::VoiceStt, audit)
        .await;
    Ok(Json(reply))
}

#[derive(Deserialize)]
struct SpeakBody {
    text: String,
    /// Speak with this persona's voice instead of the caller's own (to preview a voice).
    #[serde(default)]
    persona: Option<String>,
}

/// `POST /api/voice/speak`
pub async fn speak(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Response, ApiError> {
    let voice = state.voice();
    if !voice.tts_ready() {
        return Err(unavailable("text-to-speech"));
    }
    let req: SpeakBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))?;
    let persona = match req
        .persona
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(id) => state
            .personas()
            .get(id)
            .map_err(|_| ApiError::BadRequest(format!("unknown persona '{id}'")))?
            .id
            .clone(),
        None => state.user_settings(session.as_ref()).await.persona,
    };
    let tts = voice
        .tts_for(&persona)
        .ok_or_else(|| unavailable("text-to-speech"))?;
    let text = req.text.trim();
    if text.is_empty() {
        return Err(ApiError::BadRequest("text must not be empty".to_owned()));
    }
    let chars = text.chars().count();
    if chars > MAX_SPEAK_CHARS {
        return Err(ApiError::BadRequest(format!(
            "text is too long (maximum {MAX_SPEAK_CHARS} characters)"
        )));
    }

    let _permit = voice
        .tts_gate
        .acquire()
        .await
        .map_err(|_| ApiError::Internal)?;
    let wav = tts.synthesize(text).await.map_err(|e| {
        tracing::error!("text-to-speech failed: {e}");
        ApiError::Internal
    })?;
    drop(_permit);

    state
        .audit()
        .record_as(
            None,
            Some(&persona),
            AuditKind::VoiceTts,
            json!({ "text": text, "chars": chars, "bytes": wav.len(), "persona": persona }),
        )
        .await;
    let mut response = (StatusCode::OK, wav).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    Ok(response)
}
