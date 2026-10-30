//! Text-to-speech with Piper, run as a child process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::{TextToSpeech, VoiceConfig, VoiceError};

/// Longest text sent to Piper, in characters.
pub const MAX_SPEECH_CHARS: usize = 600;

/// How long one synthesis may take before the process is killed.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// How much of Piper's stderr is kept for error messages.
const STDERR_TAIL_BYTES: usize = 800;

/// Piper TTS: spawns `piper --model <voice> --output_file <tmp.wav>` with the text on
/// stdin, with a timeout, and returns the WAV bytes. Text is length-limited and
/// stripped of control characters before it reaches the process.
///
/// Each call starts a fresh process (Piper loads the voice in about a quarter of a
/// second), so the value is cheap to build, `Send + Sync`, and calls can overlap.
#[derive(Debug, Clone)]
pub struct PiperTts {
    exe: PathBuf,
    voice: PathBuf,
    timeout: Duration,
}

impl PiperTts {
    /// Checks that the executable, the voice `.onnx` and its `.onnx.json` exist.
    pub fn new(config: &VoiceConfig) -> Result<Self, VoiceError> {
        let exe = require_file(&config.piper_exe, "piper_exe")?;
        let voice = require_file(&config.piper_voice, "piper_voice")?;
        let voice_json = voice_config_path(&voice);
        require_file(
            &voice_json,
            "the voice config (.onnx.json next to piper_voice)",
        )?;
        Ok(Self {
            exe,
            voice,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Override the 20 s synthesis timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl TextToSpeech for PiperTts {
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
        let text = sanitize_text(text)?;
        let out = TempWav::new();

        let mut cmd = Command::new(&self.exe);
        cmd.args(piper_args(&self.voice, out.path()))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: no console flashes up when run from a service or the GUI.
            cmd.creation_flags(0x0800_0000);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("cannot start {}: {e}", self.exe.display())))?;
        let mut stdin = child.stdin.take();
        let mut stderr = child.stderr.take();

        let run = async {
            // Stdin and stderr are handled concurrently with the wait so a full pipe
            // can never stall the process.
            let write = async {
                if let Some(mut stdin) = stdin.take() {
                    stdin.write_all(text.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.shutdown().await?;
                    // Dropping closes the pipe: Piper sees end of input.
                }
                Ok::<(), std::io::Error>(())
            };
            let read_err = async {
                let mut tail = Vec::new();
                if let Some(stderr) = stderr.as_mut() {
                    let mut buf = [0u8; 4096];
                    loop {
                        match stderr.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                tail.extend_from_slice(&buf[..n]);
                                if tail.len() > STDERR_TAIL_BYTES * 4 {
                                    let cut = tail.len() - STDERR_TAIL_BYTES;
                                    tail.drain(..cut);
                                }
                            }
                        }
                    }
                }
                tail
            };
            let (written, tail, status) = tokio::join!(write, read_err, child.wait());
            (written, tail, status)
        };

        let (written, tail, status) = match tokio::time::timeout(self.timeout, run).await {
            Ok(done) => done,
            Err(_) => {
                // Dropping `child` (kill_on_drop) ends the process.
                return Err(VoiceError::Tts(format!(
                    "piper did not finish within {} s",
                    self.timeout.as_secs()
                )));
            }
        };

        let status =
            status.map_err(|e| VoiceError::Tts(format!("waiting for piper failed: {e}")))?;
        let stderr_text = tail_text(&tail);
        if !status.success() {
            return Err(VoiceError::Tts(format!(
                "piper exited with {status}{}",
                with_detail(&stderr_text)
            )));
        }
        if let Err(e) = written {
            // Piper may close stdin early after a failure; report it as such.
            return Err(VoiceError::Tts(format!(
                "could not send text to piper: {e}{}",
                with_detail(&stderr_text)
            )));
        }

        let wav = tokio::fs::read(out.path()).await.map_err(|e| {
            VoiceError::Tts(format!(
                "piper produced no output file: {e}{}",
                with_detail(&stderr_text)
            ))
        })?;
        if wav.len() < 44 || &wav[..4] != b"RIFF" {
            return Err(VoiceError::Tts(format!(
                "piper output is not a WAV file ({} bytes){}",
                wav.len(),
                with_detail(&stderr_text)
            )));
        }
        Ok(wav)
    }
}

// --- Pure helpers ------------------------------------------------------------------

/// Text safe to send to Piper: control characters removed (line breaks and tabs become
/// spaces so words do not run together), whitespace collapsed, capped at
/// [`MAX_SPEECH_CHARS`] characters. Empty input is an error.
pub(crate) fn sanitize_text(text: &str) -> Result<String, VoiceError> {
    let cleaned: String = text
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if c.is_control() {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = collapsed.chars().take(MAX_SPEECH_CHARS).collect();
    let capped = capped.trim_end().to_owned();
    if capped.is_empty() {
        return Err(VoiceError::Tts("nothing to say: text is empty".to_owned()));
    }
    Ok(capped)
}

/// Arguments for the Piper process, one item per argument (no shell involved).
pub(crate) fn piper_args(voice: &Path, output_file: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--model"),
        voice.as_os_str().to_owned(),
        OsString::from("--output_file"),
        output_file.as_os_str().to_owned(),
    ]
}

/// `voice.onnx` → `voice.onnx.json`.
fn voice_config_path(voice: &Path) -> PathBuf {
    let mut p = voice.as_os_str().to_owned();
    p.push(".json");
    PathBuf::from(p)
}

fn require_file(path: &Path, what: &str) -> Result<PathBuf, VoiceError> {
    if !path.is_file() {
        return Err(VoiceError::Config(format!(
            "{what} not found: {}",
            path.display()
        )));
    }
    // An absolute path keeps working whatever the child's working directory is.
    std::path::absolute(path)
        .map_err(|e| VoiceError::Config(format!("{what}: cannot resolve {}: {e}", path.display())))
}

fn tail_text(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}

fn with_detail(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// A unique path in the system temp directory, deleted when dropped.
struct TempWav {
    path: PathBuf,
}

impl TempWav {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(
            "jarvis-piper-{}-{}-{nanos}.wav",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            path: std::env::temp_dir().join(name),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_trims_and_collapses_whitespace() {
        assert_eq!(sanitize_text("  hello   world \n").unwrap(), "hello world");
    }

    #[test]
    fn sanitize_turns_line_breaks_into_spaces() {
        assert_eq!(
            sanitize_text("one\ntwo\r\nthree\tfour").unwrap(),
            "one two three four"
        );
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(
            sanitize_text("he\u{0}ll\u{7}o\u{1b}[31m wor\u{8}ld").unwrap(),
            "hello[31m world"
        );
    }

    #[test]
    fn sanitize_caps_length_in_characters() {
        let long = "word ".repeat(400);
        let out = sanitize_text(&long).unwrap();
        assert!(out.chars().count() <= MAX_SPEECH_CHARS);
        assert!(out.starts_with("word word"));
        assert!(!out.ends_with(' '));
        let multibyte = "\u{e9}".repeat(1000);
        assert_eq!(
            sanitize_text(&multibyte).unwrap().chars().count(),
            MAX_SPEECH_CHARS
        );
    }

    #[test]
    fn sanitize_keeps_text_at_the_limit() {
        let exact = "a".repeat(MAX_SPEECH_CHARS);
        assert_eq!(sanitize_text(&exact).unwrap(), exact);
    }

    #[test]
    fn sanitize_rejects_empty() {
        for s in ["", "   ", "\n\t", "\u{0}\u{1}\u{7}"] {
            assert!(
                matches!(sanitize_text(s), Err(VoiceError::Tts(_))),
                "{s:?} should be rejected"
            );
        }
    }

    #[test]
    fn sanitize_passes_shell_metacharacters_through_as_plain_text() {
        // Text goes to stdin, never to a command line.
        assert_eq!(
            sanitize_text("sell & close; rm -rf / $(whoami) `x` | y > z").unwrap(),
            "sell & close; rm -rf / $(whoami) `x` | y > z"
        );
    }

    #[test]
    fn args_are_separate_items() {
        let args = piper_args(
            Path::new("C:/a b/voice.onnx"),
            Path::new("C:/tmp/out file.wav"),
        );
        assert_eq!(
            args,
            vec![
                OsString::from("--model"),
                OsString::from("C:/a b/voice.onnx"),
                OsString::from("--output_file"),
                OsString::from("C:/tmp/out file.wav"),
            ]
        );
    }

    #[test]
    fn voice_config_sits_next_to_the_model() {
        assert_eq!(
            voice_config_path(Path::new("models/piper/en.onnx")),
            PathBuf::from("models/piper/en.onnx.json")
        );
    }

    #[test]
    fn temp_names_are_unique_and_removed_on_drop() {
        let a = TempWav::new();
        let b = TempWav::new();
        assert_ne!(a.path(), b.path());
        std::fs::write(a.path(), b"x").unwrap();
        let p = a.path().to_owned();
        drop(a);
        assert!(!p.exists());
    }

    // --- constructor checks ---

    fn test_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "jarvis-voice-test-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config_in(dir: &Path, exe: &str) -> VoiceConfig {
        VoiceConfig {
            enabled: true,
            whisper_model: dir.join("model.bin"),
            language: "en".into(),
            threads: 2,
            piper_exe: dir.join(exe),
            piper_voice: dir.join("voice.onnx"),
            max_seconds: 30.0,
        }
    }

    #[test]
    fn new_reports_each_missing_file() {
        let dir = test_dir("new");
        let cfg = config_in(&dir, "piper.exe");
        let msg = |r: Result<PiperTts, VoiceError>| match r {
            Err(VoiceError::Config(m)) => m,
            other => panic!("expected a config error, got {other:?}"),
        };

        assert!(msg(PiperTts::new(&cfg)).contains("piper_exe"));
        std::fs::write(&cfg.piper_exe, b"x").unwrap();
        assert!(msg(PiperTts::new(&cfg)).contains("piper_voice"));
        std::fs::write(&cfg.piper_voice, b"x").unwrap();
        assert!(msg(PiperTts::new(&cfg)).contains(".onnx.json"));
        std::fs::write(dir.join("voice.onnx.json"), b"{}").unwrap();
        assert!(PiperTts::new(&cfg).is_ok());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // --- fake piper (a batch file), Windows only ---

    #[cfg(windows)]
    mod fake {
        use super::*;
        use std::io::Cursor;
        use std::time::Instant;

        /// The tests count temp files by process id, so they run one at a time.
        static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

        /// Saves the script directory in `!HERE!` (SHIFT would break `%~dp0`), gives the
        /// system tools by full path (under cargo the test PATH is too long for cmd.exe
        /// to search) and finds the `--output_file` argument, leaving it in `!OUT!`.
        const PROLOGUE: &str = "@echo off\r\nsetlocal enabledelayedexpansion\r\nset \"HERE=%~dp0\"\r\nset \"PING=%SystemRoot%\\System32\\ping.exe\"\r\nset \"OUT=\"\r\n:loop\r\nif \"%~1\"==\"\" goto done\r\nif \"%~1\"==\"--output_file\" (\r\n  set \"OUT=%~2\"\r\n  shift\r\n)\r\nshift\r\ngoto loop\r\n:done\r\n";

        fn tiny_wav() -> Vec<u8> {
            let mut cur = Cursor::new(Vec::new());
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 22_050,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
            for i in 0..2205 {
                w.write_sample((i % 100) as i16).unwrap();
            }
            w.finalize().unwrap();
            cur.into_inner()
        }

        /// A directory with a fake `piper.cmd` whose behaviour after the prologue is
        /// `body`, plus the voice files and `fake.wav`.
        fn setup(tag: &str, body: &str) -> (PathBuf, PiperTts) {
            let dir = test_dir(tag);
            std::fs::write(dir.join("piper.cmd"), format!("{PROLOGUE}{body}\r\n")).unwrap();
            std::fs::write(dir.join("voice.onnx"), b"x").unwrap();
            std::fs::write(dir.join("voice.onnx.json"), b"{}").unwrap();
            std::fs::write(dir.join("fake.wav"), tiny_wav()).unwrap();
            let tts = PiperTts::new(&config_in(&dir, "piper.cmd")).unwrap();
            (dir, tts)
        }

        fn leftover_outputs() -> usize {
            let prefix = format!("jarvis-piper-{}-", std::process::id());
            std::fs::read_dir(std::env::temp_dir())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                .count()
        }

        #[tokio::test]
        async fn synthesize_returns_the_wav_and_sends_clean_text() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup(
                "ok",
                "set /p \"LINE=\"\r\n>\"!HERE!stdin.txt\" echo !LINE!\r\ncopy /b \"!HERE!fake.wav\" \"!OUT!\" >nul\r\nexit /b 0",
            );
            let wav = tts.synthesize("  Buy\nnow \u{7}please  ").await.unwrap();
            assert_eq!(wav, tiny_wav());
            let stdin = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
            assert_eq!(stdin.trim(), "Buy now please");
            assert_eq!(leftover_outputs(), 0, "temp output must be deleted");
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn empty_text_never_starts_the_process() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup("empty", "exit /b 0");
            let err = tts.synthesize(" \n ").await.unwrap_err();
            assert!(matches!(err, VoiceError::Tts(m) if m.contains("empty")));
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn nonzero_exit_reports_stderr() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup("fail", "echo voice model is broken 1>&2\r\nexit /b 3");
            let err = tts.synthesize("hello").await.unwrap_err();
            let VoiceError::Tts(m) = err else {
                panic!("wrong error")
            };
            assert!(m.contains("voice model is broken"), "{m}");
            assert!(m.contains("exit"), "{m}");
            assert_eq!(leftover_outputs(), 0);
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn success_without_output_file_is_an_error() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup("nofile", "more > nul\r\nexit /b 0");
            let err = tts.synthesize("hello").await.unwrap_err();
            assert!(matches!(err, VoiceError::Tts(m) if m.contains("no output file")));
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn non_wav_output_is_an_error() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup(
                "notwav",
                "more > nul\r\necho not audio > \"!OUT!\"\r\nexit /b 0",
            );
            let err = tts.synthesize("hello").await.unwrap_err();
            assert!(matches!(err, VoiceError::Tts(m) if m.contains("not a WAV")));
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[tokio::test]
        async fn slow_process_is_killed_at_the_timeout() {
            let _serial = SERIAL.lock().await;
            let (dir, tts) = setup("slow", "\"!PING!\" -n 8 127.0.0.1 >nul\r\nexit /b 0");
            let tts = tts.with_timeout(Duration::from_millis(400));
            let started = Instant::now();
            let err = tts.synthesize("hello").await.unwrap_err();
            assert!(
                matches!(&err, VoiceError::Tts(m) if m.contains("did not finish")),
                "{err:?}"
            );
            assert!(started.elapsed() < Duration::from_secs(5));
            std::fs::remove_dir_all(&dir).ok();
        }

        #[tokio::test]
        async fn missing_executable_after_construction_is_an_error() {
            let _serial = SERIAL.lock().await;
            let dir = test_dir("gone");
            std::fs::write(dir.join("piper.exe"), b"x").unwrap();
            std::fs::write(dir.join("voice.onnx"), b"x").unwrap();
            std::fs::write(dir.join("voice.onnx.json"), b"{}").unwrap();
            let tts = PiperTts::new(&config_in(&dir, "piper.exe")).unwrap();
            std::fs::remove_file(dir.join("piper.exe")).unwrap();
            let err = tts.synthesize("hello").await.unwrap_err();
            assert!(matches!(err, VoiceError::Tts(m) if m.contains("cannot start")));
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn tail_keeps_only_the_end() {
        let big = "x".repeat(5000) + "END";
        let t = tail_text(big.as_bytes());
        assert!(t.len() <= STDERR_TAIL_BYTES);
        assert!(t.ends_with("END"));
    }
}
