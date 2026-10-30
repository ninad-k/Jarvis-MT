//! WAV decoding and resampling to the 16 kHz mono f32 that Whisper expects.

use std::io::Cursor;

use hound::{SampleFormat, WavReader};

use crate::VoiceError;

/// Sample rate Whisper expects.
pub const TARGET_RATE: u32 = 16_000;

/// Highest input rate accepted; anything above is almost certainly a corrupt header.
const MAX_INPUT_RATE: u32 = 192_000;

/// Decode a 16-bit PCM (or 32-bit float) WAV into 16 kHz mono f32 samples in [-1, 1].
/// Rejects other formats and clips longer than `max_seconds`.
///
/// Stereo is averaged to mono. Other sample rates are converted with a windowed-sinc
/// resampler that low-passes below the new Nyquist frequency first.
pub fn decode_wav(bytes: &[u8], max_seconds: f32) -> Result<Vec<f32>, VoiceError> {
    if !max_seconds.is_finite() || max_seconds <= 0.0 {
        return Err(VoiceError::Config(format!(
            "max_seconds must be positive, got {max_seconds}"
        )));
    }
    if bytes.is_empty() {
        return Err(VoiceError::Audio("empty audio".to_owned()));
    }

    let mut reader = WavReader::new(Cursor::new(bytes))
        .map_err(|e| VoiceError::Audio(format!("not a readable WAV file: {e}")))?;
    let spec = reader.spec();

    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) | (SampleFormat::Float, 32) => {}
        (format, bits) => {
            return Err(VoiceError::Audio(format!(
                "unsupported WAV format ({format:?}, {bits}-bit); use 16-bit PCM or 32-bit float"
            )));
        }
    }
    if !(1..=2).contains(&spec.channels) {
        return Err(VoiceError::Audio(format!(
            "unsupported channel count {}; use mono or stereo",
            spec.channels
        )));
    }
    if spec.sample_rate < 1_000 || spec.sample_rate > MAX_INPUT_RATE {
        return Err(VoiceError::Audio(format!(
            "unsupported sample rate {} Hz",
            spec.sample_rate
        )));
    }

    let channels = usize::from(spec.channels);
    let frames = reader.duration() as usize;
    if frames == 0 {
        return Err(VoiceError::Audio("audio contains no samples".to_owned()));
    }
    let seconds = frames as f64 / f64::from(spec.sample_rate);
    if seconds > f64::from(max_seconds) {
        return Err(VoiceError::Audio(format!(
            "clip is {seconds:.1} s long; the limit is {max_seconds} s"
        )));
    }

    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32768.0))
            .collect::<Result<_, _>>(),
        SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| {
                s.map(|v| {
                    if v.is_finite() {
                        v.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
            })
            .collect::<Result<_, _>>(),
    }
    .map_err(|e| VoiceError::Audio(format!("corrupt WAV data: {e}")))?;

    let mono: Vec<f32> = if channels == 1 {
        interleaved
    } else {
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    if mono.is_empty() {
        return Err(VoiceError::Audio("audio contains no samples".to_owned()));
    }

    Ok(resample(&mono, spec.sample_rate, TARGET_RATE))
}

/// Zero crossings of the sinc kernel on each side of the centre.
const KERNEL_LOBES: f64 = 16.0;
/// Cut-off as a fraction of the lower of the two Nyquist frequencies.
const CUTOFF: f64 = 0.98;

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Blackman window over `|x| < half` (zero outside).
fn blackman(x: f64, half: f64) -> f64 {
    if x.abs() >= half {
        return 0.0;
    }
    let t = std::f64::consts::PI * x / half;
    0.42 + 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
}

/// Band-limited resampling of `input` from `from` Hz to `to` Hz (windowed sinc,
/// polyphase kernels cached per output phase, zero padding at the edges).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let g = gcd(from, to);
    let up = (to / g) as u64; // output samples per period
    let down = (from / g) as u64; // input samples per period

    // Low-pass at the lower Nyquist; the kernel widens when downsampling.
    let fc = CUTOFF * (f64::from(to) / f64::from(from)).min(1.0);
    let half = KERNEL_LOBES / fc;
    let reach = half.ceil() as i64;
    let taps = (2 * reach) as usize;

    let out_len = (input.len() as u64 * up / down) as usize;
    let mut out = Vec::with_capacity(out_len);
    let mut kernels: Vec<Option<Vec<f32>>> = vec![None; up as usize];

    for j in 0..out_len as u64 {
        let pos = j * down;
        let centre = (pos / up) as i64;
        let phase = (pos % up) as usize;
        let frac = phase as f64 / up as f64;

        let kernel = kernels[phase].get_or_insert_with(|| {
            let mut k: Vec<f64> = (0..taps as i64)
                .map(|m| {
                    // Input index n = centre - reach + 1 + m, at distance x from the output instant.
                    let x = (m - reach + 1) as f64 - frac;
                    fc * sinc(fc * x) * blackman(x, half)
                })
                .collect();
            let sum: f64 = k.iter().sum();
            if sum.abs() > 1e-12 {
                k.iter_mut().for_each(|v| *v /= sum);
            }
            k.into_iter().map(|v| v as f32).collect()
        });

        let first = centre - reach + 1;
        let mut acc = 0.0f32;
        for (m, w) in kernel.iter().enumerate() {
            let n = first + m as i64;
            if n >= 0 && (n as usize) < input.len() {
                acc += w * input[n as usize];
            }
        }
        out.push(acc.clamp(-1.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{WavSpec, WavWriter};

    fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn wav_i16(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
        let mut cur = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::new(&mut cur, spec).unwrap();
        for &s in samples {
            w.write_sample((s * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        cur.into_inner()
    }

    fn wav_f32(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
        let mut cur = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut w = WavWriter::new(&mut cur, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
        cur.into_inner()
    }

    /// Peak absolute value ignoring `skip` samples at each end (filter edge effects).
    fn peak(x: &[f32], skip: usize) -> f32 {
        x[skip..x.len() - skip]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn stereo_44k_sine_becomes_mono_16k() {
        let mono = sine(440.0, 44_100, 1.0, 0.5);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let out = decode_wav(&wav_i16(&stereo, 44_100, 2), 30.0).unwrap();
        assert_eq!(out.len(), 16_000);
        let p = peak(&out, 400);
        assert!((p - 0.5).abs() < 0.02, "peak {p}");
        // Frequency preserved: the signal still resembles a 440 Hz sine.
        let reference = sine(440.0, 16_000, 1.0, 0.5);
        let err = rms(&out
            .iter()
            .zip(&reference)
            .skip(400)
            .take(15_000)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        assert!(err < 0.02, "rms error {err}");
    }

    #[test]
    fn mono_48k_length_and_amplitude() {
        let x = sine(1000.0, 48_000, 2.0, 0.8);
        let out = decode_wav(&wav_i16(&x, 48_000, 1), 30.0).unwrap();
        assert_eq!(out.len(), 32_000);
        assert!((peak(&out, 400) - 0.8).abs() < 0.02);
    }

    #[test]
    fn upsample_8k_to_16k() {
        let x = sine(500.0, 8_000, 1.0, 0.6);
        let out = decode_wav(&wav_i16(&x, 8_000, 1), 30.0).unwrap();
        assert_eq!(out.len(), 16_000);
        assert!((peak(&out, 400) - 0.6).abs() < 0.02);
    }

    #[test]
    fn already_16k_is_passed_through() {
        let x = sine(300.0, 16_000, 0.5, 0.4);
        let out = decode_wav(&wav_i16(&x, 16_000, 1), 30.0).unwrap();
        assert_eq!(out.len(), 8_000);
        let err = x
            .iter()
            .zip(&out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn float_wav_is_accepted() {
        let x = sine(440.0, 22_050, 1.0, 0.7);
        let out = decode_wav(&wav_f32(&x, 22_050, 1), 30.0).unwrap();
        assert_eq!(out.len(), 16_000);
        assert!((peak(&out, 400) - 0.7).abs() < 0.02);
    }

    #[test]
    fn out_of_band_tone_is_attenuated_when_downsampling() {
        // 12 kHz is above the 8 kHz Nyquist of the output and must not alias into band.
        let x = sine(12_000.0, 48_000, 1.0, 0.8);
        let out = decode_wav(&wav_i16(&x, 48_000, 1), 30.0).unwrap();
        assert!(peak(&out, 600) < 0.02, "peak {}", peak(&out, 600));
    }

    #[test]
    fn stereo_channels_are_averaged() {
        let left = sine(440.0, 16_000, 0.5, 0.8);
        let interleaved: Vec<f32> = left.iter().flat_map(|&s| [s, -s]).collect();
        let out = decode_wav(&wav_i16(&interleaved, 16_000, 2), 30.0).unwrap();
        assert!(peak(&out, 0) < 1e-3);
    }

    #[test]
    fn odd_rate_works() {
        let x = sine(440.0, 11_025, 1.0, 0.5);
        let out = decode_wav(&wav_i16(&x, 11_025, 1), 30.0).unwrap();
        assert_eq!(out.len(), 16_000);
        assert!((peak(&out, 400) - 0.5).abs() < 0.02);
    }

    #[test]
    fn rejects_too_long() {
        let x = sine(440.0, 16_000, 3.0, 0.5);
        let err = decode_wav(&wav_i16(&x, 16_000, 1), 2.0).unwrap_err();
        assert!(matches!(err, VoiceError::Audio(m) if m.contains("limit")));
        assert!(decode_wav(&wav_i16(&x, 16_000, 1), 3.5).is_ok());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(decode_wav(&[], 30.0), Err(VoiceError::Audio(_))));
    }

    #[test]
    fn rejects_wav_without_samples() {
        let err = decode_wav(&wav_i16(&[], 16_000, 1), 30.0).unwrap_err();
        assert!(matches!(err, VoiceError::Audio(_)));
    }

    #[test]
    fn rejects_garbage() {
        let err = decode_wav(b"this is not a wav file at all", 30.0).unwrap_err();
        assert!(matches!(err, VoiceError::Audio(_)));
    }

    #[test]
    fn rejects_unsupported_formats() {
        // 24-bit int PCM.
        let mut cur = Cursor::new(Vec::new());
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 24,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::new(&mut cur, spec).unwrap();
        w.write_sample(0i32).unwrap();
        w.finalize().unwrap();
        assert!(matches!(
            decode_wav(&cur.into_inner(), 30.0),
            Err(VoiceError::Audio(m)) if m.contains("unsupported WAV format")
        ));
    }

    #[test]
    fn rejects_more_than_two_channels() {
        let x = vec![0.1f32; 600];
        let err = decode_wav(&wav_i16(&x, 16_000, 3), 30.0).unwrap_err();
        assert!(matches!(err, VoiceError::Audio(m) if m.contains("channel")));
    }

    #[test]
    fn rejects_truncated_data() {
        let x = sine(440.0, 16_000, 1.0, 0.5);
        let mut bytes = wav_i16(&x, 16_000, 1);
        bytes.truncate(bytes.len() / 2);
        assert!(matches!(
            decode_wav(&bytes, 30.0),
            Err(VoiceError::Audio(_))
        ));
    }

    #[test]
    fn invalid_max_seconds_is_a_config_error() {
        let x = sine(440.0, 16_000, 0.5, 0.5);
        let bytes = wav_i16(&x, 16_000, 1);
        assert!(matches!(
            decode_wav(&bytes, 0.0),
            Err(VoiceError::Config(_))
        ));
        assert!(matches!(
            decode_wav(&bytes, f32::NAN),
            Err(VoiceError::Config(_))
        ));
    }

    #[test]
    fn non_finite_float_samples_become_silence() {
        let bytes = wav_f32(&[f32::NAN, f32::INFINITY, 2.0, -2.0], 16_000, 1);
        let out = decode_wav(&bytes, 30.0).unwrap();
        assert_eq!(out, vec![0.0, 0.0, 1.0, -1.0]);
    }

    #[test]
    fn dc_gain_is_unity() {
        let out = resample(&vec![0.5f32; 44_100], 44_100, 16_000);
        assert!((peak(&out, 400) - 0.5).abs() < 0.01);
        assert!(
            out[400..out.len() - 400]
                .iter()
                .all(|v| (v - 0.5).abs() < 0.01)
        );
    }

    #[test]
    fn resample_thirty_seconds_is_fast_enough() {
        let x = vec![0.1f32; 44_100 * 30];
        let start = std::time::Instant::now();
        let out = resample(&x, 44_100, 16_000);
        assert_eq!(out.len(), 16_000 * 30);
        assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
    }
}
