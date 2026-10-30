//! Live tests with the real models from `scripts/fetch-models.ps1`.
//!
//! Run with:
//! `cargo test -p jarvis-voice --features whisper --test live -- --ignored --nocapture`

use std::path::PathBuf;
use std::time::Instant;

use jarvis_voice::{PiperTts, TextToSpeech, VoiceConfig, decode_wav};

const PHRASE: &str = "Buy point one lot of gold with stop loss three nine nine zero";

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models")
}

fn config() -> VoiceConfig {
    let m = models_dir();
    VoiceConfig {
        enabled: true,
        whisper_model: m.join("whisper/ggml-base.en.bin"),
        language: "en".to_owned(),
        threads: 4,
        piper_exe: m.join("piper/piper.exe"),
        piper_voice: m.join("piper/en_US-lessac-medium.onnx"),
        max_seconds: 30.0,
    }
}

#[tokio::test]
#[ignore = "needs models from scripts/fetch-models.ps1"]
async fn piper_speaks_and_the_wav_decodes() {
    let tts = PiperTts::new(&config()).expect("piper files present");

    let t = Instant::now();
    let wav = tts.synthesize(PHRASE).await.expect("synthesis");
    println!(
        "piper synthesize: {} ms, {} bytes",
        t.elapsed().as_millis(),
        wav.len()
    );
    assert_eq!(&wav[..4], b"RIFF");

    let t = Instant::now();
    let samples = decode_wav(&wav, 30.0).expect("decode");
    println!(
        "decode_wav: {} ms, {} samples = {:.2} s at 16 kHz",
        t.elapsed().as_millis(),
        samples.len(),
        samples.len() as f32 / 16_000.0
    );
    assert!(samples.len() > 16_000);
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.1, "audio looks silent, peak {peak}");
}

#[cfg(feature = "whisper")]
#[tokio::test]
#[ignore = "needs models from scripts/fetch-models.ps1"]
async fn spoken_order_is_transcribed_and_normalised() {
    use jarvis_voice::{SpeechToText, WhisperStt, default_corrections};

    let cfg = config();
    let tts = PiperTts::new(&cfg).expect("piper files present");

    let t = Instant::now();
    let wav = tts.synthesize(PHRASE).await.expect("synthesis");
    println!("piper synthesize: {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let samples = decode_wav(&wav, cfg.max_seconds).expect("decode");
    println!("decode_wav: {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let stt = WhisperStt::new(&cfg, default_corrections()).expect("whisper model loads");
    println!(
        "WhisperStt::new (model load): {} ms",
        t.elapsed().as_millis()
    );

    let mut last = None;
    for run in 1..=3 {
        let t = Instant::now();
        let tr = stt
            .transcribe(samples.clone())
            .await
            .expect("transcription");
        println!(
            "transcribe #{run}: wall {} ms (reported {} ms for {} ms of audio)\n  raw:  {:?}\n  text: {:?}",
            t.elapsed().as_millis(),
            tr.elapsed_ms,
            tr.audio_ms,
            tr.raw,
            tr.text
        );
        last = Some(tr);
    }

    let text = last.unwrap().text.to_lowercase();
    assert!(text.contains("buy"), "{text}");
    assert!(text.contains("gold"), "{text}");
    assert!(
        text.contains("0.1") || text.contains("point one") || text.contains("point 1"),
        "{text}"
    );
}
