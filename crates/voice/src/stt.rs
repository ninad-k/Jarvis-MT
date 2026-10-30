//! Speech-to-text with whisper.cpp through `whisper-rs` (cargo feature `whisper`).

use async_trait::async_trait;

use crate::{Corrections, SpeechToText, Transcript, VoiceConfig, VoiceError};

/// Whisper wants at least one second of audio; shorter clips are padded with silence.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
const MIN_SAMPLES: usize = 16_800;

/// Whisper STT. Loads the model once; transcription runs on a blocking thread.
/// Without the `whisper` feature, [`WhisperStt::new`] returns `VoiceError::NotAvailable`.
///
/// Building one reads the model file (about 150 MB for `base.en`), so do it once at
/// start-up and share it (`Arc<WhisperStt>`); it is `Send + Sync`. Calls are serialised
/// on one Whisper state, so overlapping requests wait for each other.
pub struct WhisperStt {
    #[cfg(feature = "whisper")]
    inner: std::sync::Arc<imp::Inner>,
    #[cfg(not(feature = "whisper"))]
    _private: (),
}

impl std::fmt::Debug for WhisperStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperStt").finish_non_exhaustive()
    }
}

impl WhisperStt {
    /// Loads the model named in `config.whisper_model`. A missing file is a
    /// [`VoiceError::Config`] naming the path; without the `whisper` feature the
    /// result is always [`VoiceError::NotAvailable`].
    pub fn new(config: &VoiceConfig, corrections: Corrections) -> Result<Self, VoiceError> {
        #[cfg(feature = "whisper")]
        {
            Ok(Self {
                inner: std::sync::Arc::new(imp::Inner::load(config, corrections)?),
            })
        }
        #[cfg(not(feature = "whisper"))]
        {
            let _ = (config, corrections);
            Err(VoiceError::NotAvailable)
        }
    }
}

#[async_trait]
impl SpeechToText for WhisperStt {
    async fn transcribe(&self, samples: Vec<f32>) -> Result<Transcript, VoiceError> {
        #[cfg(feature = "whisper")]
        {
            if samples.is_empty() {
                return Err(VoiceError::Audio("audio contains no samples".to_owned()));
            }
            let inner = std::sync::Arc::clone(&self.inner);
            tokio::task::spawn_blocking(move || inner.run(samples))
                .await
                .map_err(|e| VoiceError::Stt(format!("transcription task failed: {e}")))?
        }
        #[cfg(not(feature = "whisper"))]
        {
            let _ = samples;
            Err(VoiceError::NotAvailable)
        }
    }
}

/// Parse `VoiceConfig::language`: `"auto"` means detect, otherwise an ISO code.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn parse_language(language: &str) -> Result<Option<String>, VoiceError> {
    let lang = language.trim().to_ascii_lowercase();
    if lang.is_empty() || lang == "auto" {
        return Ok(None);
    }
    if (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_lowercase()) {
        return Ok(Some(lang));
    }
    Err(VoiceError::Config(format!(
        "language must be \"auto\" or a language code such as \"en\", got {language:?}"
    )))
}

/// Pad short clips with trailing silence up to [`MIN_SAMPLES`].
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn pad_to_minimum(mut samples: Vec<f32>) -> Vec<f32> {
    if samples.len() < MIN_SAMPLES {
        samples.resize(MIN_SAMPLES, 0.0);
    }
    samples
}

#[cfg(feature = "whisper")]
mod imp {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    use whisper_rs::{
        FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    };

    use super::{pad_to_minimum, parse_language};
    use crate::text::strip_annotations;
    use crate::{
        Corrections, TRADING_PROMPT, Transcript, VoiceConfig, VoiceError, normalize_transcript,
    };

    pub(super) struct Inner {
        // Kept alive for the state (which also holds a reference to the context).
        _ctx: WhisperContext,
        state: Mutex<WhisperState>,
        threads: i32,
        language: Option<String>,
        corrections: Corrections,
    }

    impl Inner {
        pub(super) fn load(
            config: &VoiceConfig,
            corrections: Corrections,
        ) -> Result<Self, VoiceError> {
            let path = &config.whisper_model;
            if !path.is_file() {
                return Err(VoiceError::Config(format!(
                    "whisper_model not found: {} (run scripts/fetch-models.ps1)",
                    path.display()
                )));
            }
            if config.threads == 0 {
                return Err(VoiceError::Config("threads must be at least 1".to_owned()));
            }
            let threads = i32::try_from(config.threads.min(64)).unwrap_or(4);
            let language = parse_language(&config.language)?;

            // Route whisper.cpp / ggml logs to `tracing` instead of stderr.
            whisper_rs::install_logging_hooks();

            // CPU unless whisper-rs itself is built with a GPU feature.
            let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
                .map_err(|e| {
                    VoiceError::Config(format!("cannot load whisper model {}: {e}", path.display()))
                })?;
            let state = ctx
                .create_state()
                .map_err(|e| VoiceError::Stt(format!("cannot create whisper state: {e}")))?;
            tracing::info!(model = %path.display(), threads, "whisper model loaded");
            Ok(Self {
                _ctx: ctx,
                state: Mutex::new(state),
                threads,
                language,
                corrections,
            })
        }

        fn lock_state(&self) -> MutexGuard<'_, WhisperState> {
            // A panic in an earlier call leaves the state reusable; do not wedge forever.
            self.state.lock().unwrap_or_else(|p| p.into_inner())
        }

        pub(super) fn run(&self, samples: Vec<f32>) -> Result<Transcript, VoiceError> {
            let audio_ms = (samples.len() as u64) * 1000 / 16_000;
            let samples = pad_to_minimum(samples);
            let started = Instant::now();

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(self.threads);
            params.set_language(self.language.as_deref());
            params.set_translate(false);
            params.set_no_context(true);
            params.set_no_timestamps(true);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            // Deterministic output: no temperature fallback re-decodes.
            params.set_temperature_inc(0.0);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_initial_prompt(TRADING_PROMPT);

            let mut state = self.lock_state();
            state
                .full(params, &samples)
                .map_err(|e| VoiceError::Stt(format!("whisper failed: {e}")))?;

            let mut raw = String::new();
            for segment in state.as_iter() {
                let piece = segment
                    .to_str_lossy()
                    .map_err(|e| VoiceError::Stt(format!("cannot read segment: {e}")))?;
                raw.push_str(&piece);
            }
            drop(state);

            let raw = raw.trim().to_owned();
            let text = normalize_transcript(&strip_annotations(&raw), &self.corrections);
            Ok(Transcript {
                text,
                raw,
                audio_ms,
                elapsed_ms: started.elapsed().as_millis() as u64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_parsing() {
        assert_eq!(parse_language("en").unwrap(), Some("en".to_owned()));
        assert_eq!(parse_language(" EN ").unwrap(), Some("en".to_owned()));
        assert_eq!(parse_language("auto").unwrap(), None);
        assert_eq!(parse_language("").unwrap(), None);
        assert_eq!(parse_language("hi").unwrap(), Some("hi".to_owned()));
        assert!(matches!(
            parse_language("english"),
            Err(VoiceError::Config(_))
        ));
        assert!(matches!(parse_language("e1"), Err(VoiceError::Config(_))));
    }

    #[test]
    fn short_clips_are_padded() {
        let out = pad_to_minimum(vec![0.5; 100]);
        assert_eq!(out.len(), MIN_SAMPLES);
        assert_eq!(out[99], 0.5);
        assert_eq!(out[100], 0.0);
        assert_eq!(pad_to_minimum(vec![0.1; 20_000]).len(), 20_000);
    }

    fn config() -> VoiceConfig {
        VoiceConfig {
            enabled: true,
            whisper_model: "does/not/exist.bin".into(),
            language: "en".into(),
            threads: 4,
            piper_exe: "piper.exe".into(),
            piper_voice: "voice.onnx".into(),
            max_seconds: 30.0,
        }
    }

    #[cfg(not(feature = "whisper"))]
    #[tokio::test]
    async fn without_feature_new_is_not_available() {
        let err = WhisperStt::new(&config(), Vec::new()).err().unwrap();
        assert!(matches!(err, VoiceError::NotAvailable));
    }

    #[cfg(feature = "whisper")]
    #[test]
    fn missing_model_is_a_config_error_naming_the_path() {
        let err = WhisperStt::new(&config(), Vec::new()).err().unwrap();
        match err {
            VoiceError::Config(m) => assert!(m.contains("does/not/exist.bin"), "{m}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(feature = "whisper")]
    #[test]
    fn zero_threads_and_bad_language_are_rejected() {
        let model = std::env::temp_dir().join("jarvis-voice-fake-model.bin");
        std::fs::write(&model, b"x").unwrap();
        let mut c = config();
        c.whisper_model = model.clone();
        c.threads = 0;
        assert!(matches!(
            WhisperStt::new(&c, Vec::new()),
            Err(VoiceError::Config(_))
        ));
        c.threads = 2;
        c.language = "klingon!".into();
        assert!(matches!(
            WhisperStt::new(&c, Vec::new()),
            Err(VoiceError::Config(_))
        ));
        // A file that is not a ggml model fails to load with a clear error.
        c.language = "en".into();
        assert!(matches!(
            WhisperStt::new(&c, Vec::new()),
            Err(VoiceError::Config(_))
        ));
        let _ = std::fs::remove_file(&model);
    }
}
