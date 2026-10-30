//! Persona packs: how the assistant sounds and looks, never what it may do.
//!
//! A persona is a TOML file in `personas/` (see `personas/jarvis.toml`): an id, a display
//! name, a personality prompt, a Piper voice, a wake phrase and HUD colours. Tools, limits,
//! confirmation tokens, the kill switch and every safety rule are shared and live in code;
//! [`compose_system_prompt`] places the personality *inside* a fixed frame so it cannot
//! replace those rules (ADR-0017).
//!
//! CONTRACT STUB: public types and signatures are fixed; bodies are implemented in the
//! persona task.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Longest personality prompt accepted, in characters.
pub const MAX_PERSONALITY_CHARS: usize = 4000;

/// One persona pack, as loaded from `personas/<id>.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaPack {
    /// `[a-z0-9_-]{1,32}`; also the file stem.
    pub id: String,
    /// Display name, 1–32 chars, e.g. "Friday".
    pub name: String,
    /// One-line description shown in the UI.
    pub description: String,
    /// Personality text (≤ [`MAX_PERSONALITY_CHARS`]).
    pub system_prompt: String,
    /// Piper `.onnx` voice model, relative to the working directory or absolute.
    pub voice_model: PathBuf,
    /// Lower-case wake phrase, 2–4 words, e.g. "hey friday".
    pub wake_word: String,
    pub theme: Theme,
}

/// HUD colours as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub accent: String,
    pub accent_soft: String,
    pub orb_core: String,
    pub orb_glow: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PersonaError {
    #[error("cannot read persona directory {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("invalid persona file {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("duplicate persona id {0:?}")]
    Duplicate(String),
    #[error("no personas found in {0}")]
    Empty(PathBuf),
    #[error("unknown persona {0:?}")]
    Unknown(String),
}

/// All loaded personas, sorted by id. Cheap to clone behind an `Arc` by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaRegistry {
    personas: Vec<PersonaPack>,
    default_id: String,
}

impl PersonaRegistry {
    /// Load every `*.toml` in `dir` (non-recursive), validate each pack (id format and
    /// equal to the file stem, lengths, hex colours, wake phrase shape, unknown keys
    /// rejected), reject duplicates, and pick `default_id` (must exist).
    /// A missing voice model file is NOT an error here (voice is optional); callers
    /// check [`PersonaPack::voice_model`] when building TTS.
    pub fn load_dir(dir: &Path, default_id: &str) -> Result<Self, PersonaError> {
        let _ = (dir, default_id);
        todo!("persona task")
    }

    /// Build from already-parsed packs (tests, embedded defaults). Same validation.
    pub fn from_packs(packs: Vec<PersonaPack>, default_id: &str) -> Result<Self, PersonaError> {
        let _ = (packs, default_id);
        todo!("persona task")
    }

    pub fn get(&self, id: &str) -> Result<&PersonaPack, PersonaError> {
        self.personas
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| PersonaError::Unknown(id.to_owned()))
    }

    pub fn default_persona(&self) -> &PersonaPack {
        // Invariant from the constructors: default_id exists.
        self.personas
            .iter()
            .find(|p| p.id == self.default_id)
            .unwrap_or(&self.personas[0])
    }

    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    pub fn list(&self) -> &[PersonaPack] {
        &self.personas
    }

    /// Which persona's wake phrase starts `transcript` (case-insensitive, tolerant of
    /// "hi"/"ok"/"hello" instead of "hey" and of punctuation), and the rest of the
    /// utterance after it. `None` when no wake phrase matches.
    pub fn match_wake_word<'a>(&'a self, transcript: &str) -> Option<(&'a PersonaPack, String)> {
        let _ = transcript;
        todo!("persona task")
    }
}

/// Build the final system prompt: an identity line ("You are <name>, ..."), then the
/// persona's personality inside a clearly delimited section, then `shared_rules`
/// verbatim, then a closing line stating that the shared rules always take precedence
/// over the personality section. The shared rules are identical for every persona.
pub fn compose_system_prompt(persona: &PersonaPack, shared_rules: &str) -> String {
    let _ = (persona, shared_rules);
    todo!("persona task")
}
