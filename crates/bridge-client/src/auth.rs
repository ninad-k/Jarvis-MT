//! Message authentication of bridge protocol 2 (ADR-0016, finding BR-1).
//!
//! Every message in both directions is a ZeroMQ multipart message of exactly two frames:
//!
//! 1. frame 1: lowercase hex of `HMAC-SHA256(key, frame 2)` (64 ASCII characters),
//! 2. frame 2: the JSON payload, byte for byte as it was sent.
//!
//! The MAC covers the raw bytes, so there is no JSON canonicalisation to get wrong. The key is
//! 32 random bytes shared by the API and the EA (hex in a key file). A request also carries
//! `"ts"` (Unix milliseconds of the sender's clock); the receiver refuses a `ts` more than
//! [`MAX_CLOCK_SKEW_MS`] away from its own clock and refuses an `id` it already saw in the last
//! [`REPLAY_WINDOW_MS`] ([`ReplayGuard`]). The EA implements the same rules in
//! `mql5/Include/Jarvis/Auth.mqh`.
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeromq::ZmqMessage;

type HmacSha256 = Hmac<Sha256>;

/// Length of the shared key in bytes.
pub const KEY_BYTES: usize = 32;
/// Length of the hex form of the key and of a MAC frame.
pub const HEX_LEN: usize = 64;
/// A request is refused when its `ts` differs from the receiver's clock by more than this.
pub const MAX_CLOCK_SKEW_MS: i64 = 30_000;
/// How long a request id is remembered: the skew window on both sides of "now".
pub const REPLAY_WINDOW_MS: i64 = 60_000;
/// Most ids remembered at once. When the table is full of ids that are still inside the window,
/// further requests are refused until the oldest expires (fail closed).
pub const REPLAY_CAPACITY: usize = 4096;
/// The only error text an unauthenticated peer ever receives.
pub const AUTH_ERROR_TEXT: &str = "authentication failed";
/// Environment variable that holds the key in hex and overrides the key file.
pub const KEY_ENV: &str = "JARVIS_BRIDGE_KEY";

/// Why a key could not be used. Never contains key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error(
        "the bridge key must be {HEX_LEN} hexadecimal characters (32 random bytes), got {0} characters"
    )]
    Length(usize),
    #[error("the bridge key contains a character that is not hexadecimal")]
    NotHex,
    #[error("the bridge key is all zeros; generate a random one with scripts/install-ea.ps1")]
    AllZero,
    #[error("the system random generator failed: {0}")]
    Random(String),
    #[error("cannot read the bridge key file {path}: {reason}")]
    Read { path: String, reason: String },
}

/// The shared secret. `Debug` never prints it; there is no `Display`.
#[derive(Clone)]
pub struct BridgeKey([u8; KEY_BYTES]);

impl fmt::Debug for BridgeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeKey(<redacted>)")
    }
}

/// Comparing two keys is not a secret-dependent branch for an attacker, but do it in constant
/// time anyway so that no caller has to think about it.
impl PartialEq for BridgeKey {
    fn eq(&self, other: &Self) -> bool {
        ct_eq(&self.0, &other.0)
    }
}
impl Eq for BridgeKey {}

impl BridgeKey {
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Result<Self, KeyError> {
        if bytes.iter().all(|b| *b == 0) {
            return Err(KeyError::AllZero);
        }
        Ok(Self(bytes))
    }

    /// Parse the 64-character hex form. Surrounding whitespace is ignored; either case is fine.
    pub fn from_hex(text: &str) -> Result<Self, KeyError> {
        let text = text.trim();
        if text.len() != HEX_LEN {
            return Err(KeyError::Length(text.chars().count()));
        }
        let mut bytes = [0u8; KEY_BYTES];
        for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            let hi = hex_val(pair[0]).ok_or(KeyError::NotHex)?;
            let lo = hex_val(pair[1]).ok_or(KeyError::NotHex)?;
            *slot = (hi << 4) | lo;
        }
        Self::from_bytes(bytes)
    }

    /// Read the key from a key file (the first non-empty line, hex).
    pub fn from_file(path: &Path) -> Result<Self, KeyError> {
        let text = std::fs::read_to_string(path).map_err(|e| KeyError::Read {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        let line = text
            .trim_start_matches('\u{feff}')
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        Self::from_hex(line)
    }

    /// A fresh random key from the operating system's generator.
    pub fn generate() -> Result<Self, KeyError> {
        let mut bytes = [0u8; KEY_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| KeyError::Random(e.to_string()))?;
        Self::from_bytes(bytes)
    }

    /// The hex form. Only for tools that must hand the key to another process (the mock server,
    /// tests). Never log the result.
    pub fn expose_hex(&self) -> String {
        hex_encode(&self.0)
    }

    fn mac(&self, payload: &[u8]) -> HmacSha256 {
        // HMAC accepts keys of any length, so this cannot fail for a 32-byte key.
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(&self.0)
            .unwrap_or_else(|_| unreachable!("HMAC takes keys of any length"));
        mac.update(payload);
        mac
    }

    /// Lowercase hex `HMAC-SHA256(key, payload)`: the content of frame 1.
    pub fn sign(&self, payload: &[u8]) -> String {
        hex_encode(&self.mac(payload).finalize().into_bytes())
    }

    /// Constant-time check of frame 1 against `payload`. Anything that is not exactly 64
    /// lowercase hex characters fails.
    pub fn verify(&self, payload: &[u8], mac_frame: &[u8]) -> bool {
        let Some(expected) = hex_decode_lower(mac_frame) else {
            return false;
        };
        self.mac(payload).verify_slice(&expected).is_ok()
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

/// Strict decoder for MAC frames: exactly 64 lowercase hex characters.
fn hex_decode_lower(frame: &[u8]) -> Option<[u8; 32]> {
    if frame.len() != HEX_LEN {
        return None;
    }
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(frame.chunks_exact(2)) {
        if pair.iter().any(u8::is_ascii_uppercase) {
            return None;
        }
        *slot = (hex_val(pair[0])? << 4) | hex_val(pair[1])?;
    }
    Some(out)
}

/// Why a message was refused. The wire never says which; logs may.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthFailure {
    #[error("not a two-frame message")]
    Frames,
    #[error("MAC does not match")]
    Mac,
    #[error("payload is not a JSON object with a string id and an integer ts")]
    Envelope,
    #[error("ts is more than 30 s away from this clock")]
    Stale,
    #[error("request id was already used in the last 60 s")]
    Replayed,
    #[error("too many requests in the last 60 s")]
    TableFull,
}

/// Frames of a signed message: `[hex MAC, payload]`.
pub fn seal(key: &BridgeKey, payload: Vec<u8>) -> ZmqMessage {
    let mac = key.sign(&payload);
    let mut message = ZmqMessage::from(Bytes::from(mac.into_bytes()));
    message.push_back(Bytes::from(payload));
    message
}

/// Check a received message and return its payload. Exactly two frames, MAC valid.
pub fn open<'a>(key: &BridgeKey, message: &'a ZmqMessage) -> Result<&'a [u8], AuthFailure> {
    if message.len() != 2 {
        return Err(AuthFailure::Frames);
    }
    let (Some(mac), Some(payload)) = (message.get(0), message.get(1)) else {
        return Err(AuthFailure::Frames);
    };
    if key.verify(payload, mac) {
        Ok(payload)
    } else {
        Err(AuthFailure::Mac)
    }
}

/// Unix milliseconds of the local clock.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The receiver's replay and freshness rules (the mock EA uses this; the EA has the same logic
/// in MQL5). Ids are remembered by arrival time on a monotonic clock, so a wall-clock jump can
/// neither expire an id early nor keep it forever.
#[derive(Debug)]
pub struct ReplayGuard {
    epoch: Instant,
    order: VecDeque<(String, i64)>,
    ids: HashSet<String>,
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    /// Check one authentic request. On success the id is remembered.
    pub fn check(&mut self, id: &str, ts_ms: i64) -> Result<(), AuthFailure> {
        let mono = i64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(i64::MAX);
        self.check_at(id, ts_ms, now_ms(), mono)
    }

    /// [`ReplayGuard::check`] with explicit clocks (tests): `wall_ms` is the receiver's Unix
    /// time, `mono_ms` any monotonic millisecond counter.
    pub fn check_at(
        &mut self,
        id: &str,
        ts_ms: i64,
        wall_ms: i64,
        mono_ms: i64,
    ) -> Result<(), AuthFailure> {
        if ts_ms.abs_diff(wall_ms) > MAX_CLOCK_SKEW_MS.unsigned_abs() {
            return Err(AuthFailure::Stale);
        }
        while let Some((oldest, at)) = self.order.front() {
            if mono_ms.saturating_sub(*at) <= REPLAY_WINDOW_MS {
                break;
            }
            self.ids.remove(oldest);
            self.order.pop_front();
        }
        if self.ids.contains(id) {
            return Err(AuthFailure::Replayed);
        }
        if self.order.len() >= REPLAY_CAPACITY {
            return Err(AuthFailure::TableFull);
        }
        self.ids.insert(id.to_owned());
        self.order.push_back((id.to_owned(), mono_ms));
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// HMAC-SHA256 through the same `BridgeKey::sign` path as production, but with an
    /// arbitrary key length, so the RFC 4231 cases (keys of 4, 20, 25 and 131 bytes) can be
    /// checked. Production keys are always 32 bytes.
    fn hmac_hex(key: &[u8], data: &[u8]) -> String {
        let mut mac = <HmacSha256 as KeyInit>::new_from_slice(key).unwrap();
        mac.update(data);
        hex_encode(&mac.finalize().into_bytes())
    }

    /// RFC 4231, section 4: cases 1, 2, 3, 4, 6 and 7 (5 is a truncated output).
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let long_key = [0xaau8; 131];
        let cases: [(Vec<u8>, Vec<u8>, &str); 6] = [
            (
                vec![0x0b; 20],
                b"Hi There".to_vec(),
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?".to_vec(),
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                vec![0xaa; 20],
                vec![0xdd; 50],
                "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
            ),
            (
                (1u8..=25).collect(),
                vec![0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
            (
                long_key.to_vec(),
                b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
            (
                long_key.to_vec(),
                b"This is a test using a larger than block-size key and a larger than \
                  block-size data. The key needs to be hashed before being used by the \
                  HMAC algorithm."
                    .to_vec(),
                "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            ),
        ];
        for (n, (key, data, want)) in cases.iter().enumerate() {
            assert_eq!(&hmac_hex(key, data), want, "RFC 4231 case {n}");
        }
    }

    #[test]
    fn hex_helpers_round_trip() {
        assert_eq!(hex_encode(&hex("00ff10ab")), "00ff10ab");
        assert!(hex_decode_lower(&[b'a'; 64]).is_some());
        assert!(hex_decode_lower(&[b'A'; 64]).is_none(), "uppercase refused");
        assert!(hex_decode_lower(&[b'a'; 63]).is_none());
        assert!(hex_decode_lower(&[b'g'; 64]).is_none());
    }

    #[test]
    fn keys_are_validated_and_never_printed() {
        let good = "ab".repeat(32);
        let key = BridgeKey::from_hex(&format!("  {good}\r\n")).unwrap();
        assert_eq!(key.expose_hex(), good);
        assert_eq!(format!("{key:?}"), "BridgeKey(<redacted>)");
        assert_eq!(BridgeKey::from_hex(&good.to_uppercase()).unwrap(), key);
        assert_eq!(BridgeKey::from_hex("abc"), Err(KeyError::Length(3)));
        assert_eq!(BridgeKey::from_hex(&"zz".repeat(32)), Err(KeyError::NotHex));
        assert_eq!(
            BridgeKey::from_hex(&"00".repeat(32)),
            Err(KeyError::AllZero)
        );
        assert_eq!(BridgeKey::from_hex(""), Err(KeyError::Length(0)));
        // The error text never contains the offending value.
        let shown = BridgeKey::from_hex(&"zz".repeat(32))
            .unwrap_err()
            .to_string();
        assert!(!shown.contains("zz"));
    }

    #[test]
    fn generated_keys_differ_and_are_valid() {
        let a = BridgeKey::generate().unwrap();
        let b = BridgeKey::generate().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.expose_hex().len(), HEX_LEN);
    }

    #[test]
    fn a_key_file_may_have_a_bom_and_blank_lines() {
        let dir = std::env::temp_dir().join(format!("jarvis-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bridge.key");
        let hex = "0f".repeat(32);
        std::fs::write(&path, format!("\u{feff}\r\n{hex}\r\n")).unwrap();
        assert_eq!(BridgeKey::from_file(&path).unwrap().expose_hex(), hex);
        std::fs::write(&path, "").unwrap();
        assert!(matches!(
            BridgeKey::from_file(&path),
            Err(KeyError::Length(0))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(
            BridgeKey::from_file(&path),
            Err(KeyError::Read { .. })
        ));
    }

    #[test]
    fn seal_and_open_round_trip_and_reject_tampering() {
        let key = BridgeKey::generate().unwrap();
        let message = seal(&key, b"{\"id\":\"x\"}".to_vec());
        assert_eq!(message.len(), 2);
        assert_eq!(open(&key, &message).unwrap(), b"{\"id\":\"x\"}");

        let other = BridgeKey::generate().unwrap();
        assert_eq!(open(&other, &message), Err(AuthFailure::Mac));

        let mut frames = message.clone().into_vec();
        frames[1] = Bytes::from_static(b"{\"id\":\"y\"}");
        let tampered = ZmqMessage::try_from(frames).unwrap();
        assert_eq!(open(&key, &tampered), Err(AuthFailure::Mac));

        let single = ZmqMessage::from(Bytes::from_static(b"{\"id\":\"x\"}"));
        assert_eq!(open(&key, &single), Err(AuthFailure::Frames));
        let mut three = message.into_vec();
        three.push(Bytes::from_static(b"extra"));
        assert_eq!(
            open(&key, &ZmqMessage::try_from(three).unwrap()),
            Err(AuthFailure::Frames)
        );
    }

    #[test]
    fn replay_guard_applies_skew_replay_expiry_and_capacity() {
        let mut g = ReplayGuard::new();
        let wall = 1_800_000_000_000;
        assert_eq!(g.check_at("a", wall, wall, 0), Ok(()));
        assert_eq!(g.check_at("a", wall, wall, 1), Err(AuthFailure::Replayed));
        // The skew window is closed at 30 s on both sides.
        assert_eq!(g.check_at("b", wall - 30_000, wall, 2), Ok(()));
        assert_eq!(g.check_at("c", wall + 30_000, wall, 3), Ok(()));
        assert_eq!(
            g.check_at("d", wall - 30_001, wall, 4),
            Err(AuthFailure::Stale)
        );
        assert_eq!(
            g.check_at("e", wall + 30_001, wall, 5),
            Err(AuthFailure::Stale)
        );
        // A refused request is not remembered.
        assert_eq!(g.check_at("d", wall, wall, 6), Ok(()));
        // After the window the id can be used again (and its old ts would be stale anyway).
        assert_eq!(
            g.check_at("a", wall, wall, REPLAY_WINDOW_MS),
            Err(AuthFailure::Replayed)
        );
        assert_eq!(g.check_at("a", wall, wall, REPLAY_WINDOW_MS + 1), Ok(()));
        // Capacity: full of fresh ids refuses, and recovers when they expire.
        let mut g = ReplayGuard::new();
        for i in 0..REPLAY_CAPACITY {
            assert_eq!(g.check_at(&format!("id{i}"), wall, wall, 10), Ok(()));
        }
        assert_eq!(
            g.check_at("one-more", wall, wall, 11),
            Err(AuthFailure::TableFull)
        );
        assert_eq!(
            g.check_at("one-more", wall, wall, 10 + REPLAY_WINDOW_MS + 1),
            Ok(())
        );
        assert_eq!(g.len(), 1);
    }
}
