//! Voice: speech-to-text and text-to-speech routes over `jarvis-voice`.
//!
//! The engines are built once at startup. Any failure there (missing model, Piper not
//! found, whisper not compiled in) is logged as a warning and leaves that half of voice
//! switched off; it never stops the server.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jarvis_gate::intent::{self, Intent};
use jarvis_voice::{
    Corrections, PiperTts, SpeechToText, TextToSpeech, VoiceConfig, VoiceError, WhisperStt,
    decode_wav, default_corrections, normalize_transcript,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::audit::AuditKind;
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
    tts: Option<Arc<dyn TextToSpeech>>,
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
            max_seconds,
            corrections: default_corrections(),
            stt_gate: Semaphore::new(1),
            tts_gate: Semaphore::new(TTS_CONCURRENCY),
        }
    }

    /// Build both engines from `[voice]`. Never fails: a broken half is left off.
    pub async fn from_config(config: &VoiceConfig) -> Self {
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
        Self::new(stt, tts, config.max_seconds)
    }

    pub fn stt_ready(&self) -> bool {
        self.stt.is_some()
    }
    pub fn tts_ready(&self) -> bool {
        self.tts.is_some()
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

/// `POST /api/voice/transcribe`
pub async fn transcribe(
    State(state): State<AppState>,
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
    let aliases = state
        .config()
        .symbols
        .aliases
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let intent = match intent::parse(&text, &aliases) {
        Intent::Confirm => "confirm",
        Intent::Cancel => "cancel",
        _ => "other",
    };
    // The text and its timing only; the audio is never stored.
    state
        .audit()
        .record(
            None,
            AuditKind::VoiceStt,
            json!({
                "text": text,
                "raw": transcript.raw,
                "audio_ms": transcript.audio_ms,
                "elapsed_ms": transcript.elapsed_ms,
                "intent": intent,
            }),
        )
        .await;
    Ok(Json(json!({
        "text": text,
        "raw": transcript.raw,
        "audio_ms": transcript.audio_ms,
        "elapsed_ms": transcript.elapsed_ms,
        "intent": intent,
    })))
}

#[derive(Deserialize)]
struct SpeakBody {
    text: String,
}

/// `POST /api/voice/speak`
pub async fn speak(State(state): State<AppState>, body: Bytes) -> Result<Response, ApiError> {
    let voice = state.voice();
    let tts = voice
        .tts
        .clone()
        .ok_or_else(|| unavailable("text-to-speech"))?;
    let req: SpeakBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))?;
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
        .record(
            None,
            AuditKind::VoiceTts,
            json!({ "text": text, "chars": chars, "bytes": wav.len() }),
        )
        .await;
    let mut response = (StatusCode::OK, wav).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    Ok(response)
}
