//! Local voice I/O: speech-to-text with whisper.cpp (via `whisper-rs`, cargo feature
//! `whisper`) and text-to-speech with Piper run as a child process.
//!
//! Audio in: 16-bit PCM WAV (the browser encodes to WAV; any sample rate, mono or
//! stereo), converted with [`decode_wav`] to 16 kHz mono f32 for Whisper.
//! Audio out: WAV bytes from Piper.
//!
//! Build notes: the `whisper` feature compiles whisper.cpp, which needs CMake and
//! LLVM (libclang) on the machine. Without it the crate still provides WAV decoding,
//! transcript clean-up and Piper TTS, and [`WhisperStt::new`] returns
//! [`VoiceError::NotAvailable`].

mod audio;
mod stt;
mod text;
mod tts;

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Trading vocabulary and common spellings given to Whisper as its initial prompt,
/// so "XAUUSD", "lot", "stop loss", "take profit", "Nasdaq" are recognised.
pub const TRADING_PROMPT: &str = "Trading commands for MetaTrader. Gold XAUUSD, silver \
XAGUSD, Nasdaq USTEC, Dow US30, S&P US500, EURUSD, GBPUSD, USDJPY, Bitcoin BTCUSD. Buy, \
sell, close, cancel, confirm, lot, lots, point one lot, stop loss, take profit, SL, TP, \
pending, limit, stop, positions, equity, balance, margin. Hinglish: kharido, becho, band \
karo, sab band karo, haan, nahi, sona, chandi.";

/// Word-level fixes applied after transcription (case-insensitive, whole words only),
/// e.g. `("cold", "gold")`, `("sell all", "sell all")`. Default list in [`default_corrections`].
pub type Corrections = Vec<(String, String)>;

pub use audio::decode_wav;
pub use stt::WhisperStt;
pub use text::{default_corrections, normalize_transcript};
pub use tts::PiperTts;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Normalised text (after corrections).
    pub text: String,
    /// Raw Whisper output, for the audit log.
    pub raw: String,
    pub audio_ms: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("voice config error: {0}")]
    Config(String),
    #[error("unsupported or invalid audio: {0}")]
    Audio(String),
    #[error("speech-to-text failed: {0}")]
    Stt(String),
    #[error("text-to-speech failed: {0}")]
    Tts(String),
    #[error("voice feature not built into this binary (enable cargo feature `whisper`)")]
    NotAvailable,
}

#[async_trait]
pub trait SpeechToText: Send + Sync {
    /// `samples`: 16 kHz mono f32 from [`decode_wav`].
    async fn transcribe(&self, samples: Vec<f32>) -> Result<Transcript, VoiceError>;
}

#[async_trait]
pub trait TextToSpeech: Send + Sync {
    /// Returns a complete WAV file.
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, VoiceError>;
}

/// `[voice]` section of `config/jarvis.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// ggml Whisper model, e.g. `models/whisper/ggml-base.en.bin`.
    pub whisper_model: PathBuf,
    /// `"en"` or `"auto"` (auto lets Hindi/Hinglish through).
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default = "default_threads")]
    pub threads: u32,
    /// Piper executable, e.g. `models/piper/piper.exe`.
    pub piper_exe: PathBuf,
    /// Piper voice model `.onnx` (its `.onnx.json` must sit next to it).
    pub piper_voice: PathBuf,
    /// Max accepted clip length.
    #[serde(default = "default_max_seconds")]
    pub max_seconds: f32,
}

fn yes() -> bool {
    true
}
fn default_lang() -> String {
    "en".to_owned()
}
fn default_threads() -> u32 {
    4
}
fn default_max_seconds() -> f32 {
    30.0
}
