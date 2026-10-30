//! Password hashing (Argon2id), TOTP helpers and the encryption of TOTP secrets at rest.

use anyhow::{anyhow, bail};
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use totp_rs::{Builder, Totp};

/// Smallest accepted JWT secret, in bytes.
pub const MIN_SECRET_BYTES: usize = 32;
/// Length of a generated TOTP secret (RFC 4226 recommends 160 bits).
const TOTP_SECRET_BYTES: usize = 20;
/// TOTP time step in seconds.
pub const TOTP_STEP_SECS: u64 = 30;
/// Name shown in the authenticator app.
const TOTP_ISSUER: &str = "Jarvis MT";
/// Prefix of a stored (encrypted) TOTP secret; lets a later format coexist.
const SEALED_PREFIX: &str = "v1:";
const HKDF_INFO: &[u8] = b"jarvis-mt/totp-secret/v1";
const HKDF_SALT: &[u8] = b"jarvis-mt";
const NONCE_BYTES: usize = 12;

/// Random bytes from the operating system.
pub fn random_bytes<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| anyhow!("the system random generator failed: {e}"))?;
    Ok(buf)
}

/// Check the signing secret: at least 32 bytes.
pub fn validate_secret(secret: &str) -> anyhow::Result<()> {
    if secret.len() < MIN_SECRET_BYTES {
        bail!(
            "JARVIS_JWT_SECRET must be at least {MIN_SECRET_BYTES} bytes (got {}). Generate one \
             with: pwsh -c \"[Convert]::ToBase64String((1..48 | % {{ Get-Random -Max 256 }}) -as \
             [byte[]])\"",
            secret.len()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

/// Argon2id with explicit cost parameters. Verification reads the parameters stored in the
/// hash, so changing the cost later does not lock anyone out.
#[derive(Clone)]
pub struct PasswordHasherCfg {
    argon: Argon2<'static>,
}

impl PasswordHasherCfg {
    /// OWASP's minimum for Argon2id: 19 MiB of memory, 2 iterations, 1 lane.
    pub fn owasp() -> Self {
        Self::with_params(19 * 1024, 2, 1)
    }

    /// Much cheaper parameters so tests do not spend seconds per login in a debug build.
    /// Never used by the server.
    pub fn cheap_for_tests() -> Self {
        Self::with_params(Params::MIN_M_COST.max(64), 1, 1)
    }

    fn with_params(m_cost: u32, t_cost: u32, p_cost: u32) -> Self {
        let params = Params::new(m_cost, t_cost, p_cost, None).expect("valid Argon2 parameters");
        Self {
            argon: Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
        }
    }

    /// PHC string with a fresh random salt.
    pub fn hash(&self, password: &str) -> anyhow::Result<String> {
        let hash = self
            .argon
            .hash_password(password.as_bytes())
            .map_err(|e| anyhow!("hashing the password failed: {e}"))?;
        Ok(hash.to_string())
    }

    /// True when `password` matches the PHC string `hash`.
    pub fn verify(&self, password: &str, hash: &str) -> bool {
        self.argon
            .verify_password(password.as_bytes(), hash)
            .is_ok()
    }
}

// ---------------------------------------------------------------------------
// TOTP
// ---------------------------------------------------------------------------

/// A new random TOTP secret.
pub fn new_totp_secret() -> anyhow::Result<Vec<u8>> {
    Ok(random_bytes::<TOTP_SECRET_BYTES>()?.to_vec())
}

/// SHA-1, 6 digits, 30 s. The skew is handled by [`matching_step`], not by the library, so
/// that the step that matched is known (replay protection).
pub fn build_totp(secret: Vec<u8>, account: &str) -> anyhow::Result<Totp> {
    Builder::new()
        .with_secret(secret)
        .with_skew(0)
        .with_issuer(Some(TOTP_ISSUER))
        .with_account_name(account)
        .build()
        .map_err(|e| anyhow!("building the TOTP generator failed: {e}"))
}

/// The time step (unix seconds / 30) whose code equals `code`, looking one step back and one
/// forward of `now_secs`. All three steps are always compared, in constant time per code.
pub fn matching_step(totp: &Totp, code: &str, now_secs: u64) -> Option<u64> {
    let current = now_secs / TOTP_STEP_SECS;
    let mut found = None;
    for step in [current.saturating_sub(1), current, current + 1] {
        if totp.check(code, step * TOTP_STEP_SECS).is_some() {
            found = Some(step);
        }
    }
    found
}

/// Digits only, exactly six: anything else cannot be a code.
pub fn looks_like_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Secrets at rest
// ---------------------------------------------------------------------------

/// Encrypts TOTP secrets with ChaCha20-Poly1305 under a key derived from the JWT secret
/// (HKDF-SHA256, fixed label), so a copy of the database alone does not reveal them. The
/// username is authenticated as associated data: a ciphertext cannot be moved to another row.
#[derive(Clone)]
pub struct SecretBox {
    cipher: ChaCha20Poly1305,
}

impl SecretBox {
    pub fn new(master_secret: &[u8]) -> anyhow::Result<Self> {
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), master_secret);
        let mut key = [0u8; 32];
        hkdf.expand(HKDF_INFO, &mut key)
            .map_err(|_| anyhow!("deriving the encryption key failed"))?;
        let key =
            Key::try_from(&key[..]).map_err(|_| anyhow!("encryption key has the wrong size"))?;
        Ok(Self {
            cipher: ChaCha20Poly1305::new(&key),
        })
    }

    pub fn seal(&self, plaintext: &[u8], context: &str) -> anyhow::Result<String> {
        let nonce_bytes = random_bytes::<NONCE_BYTES>()?;
        let nonce = Nonce::try_from(&nonce_bytes[..]).map_err(|_| anyhow!("bad nonce size"))?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: context.to_ascii_lowercase().as_bytes(),
                },
            )
            .map_err(|_| anyhow!("encrypting the TOTP secret failed"))?;
        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(format!("{SEALED_PREFIX}{}", B64.encode(blob)))
    }

    /// `None` when the text is malformed, was sealed under another key (the JWT secret
    /// changed) or for another row.
    pub fn open(&self, sealed: &str, context: &str) -> Option<Vec<u8>> {
        let blob = B64.decode(sealed.strip_prefix(SEALED_PREFIX)?).ok()?;
        if blob.len() <= NONCE_BYTES {
            return None;
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_BYTES);
        let nonce = Nonce::try_from(nonce).ok()?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: context.to_ascii_lowercase().as_bytes(),
                },
            )
            .ok()
    }
}

/// Lowest-effort context check for usernames: 1 to 64 characters of letters, digits and
/// `. _ - @`. No colon (it would break the `otpauth://` label) and no whitespace.
pub fn validate_username(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if !ok {
        bail!("a username is 1 to 64 characters from a-z, A-Z, 0-9 and . _ - @");
    }
    Ok(())
}

/// Passwords: 12 to 256 bytes (a long cap keeps hashing bounded).
pub fn validate_new_password(password: &str) -> anyhow::Result<()> {
    if password.chars().count() < 12 {
        bail!("the password must be at least 12 characters");
    }
    if password.len() > 256 {
        bail!("the password must be at most 256 bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trip_and_phc_format() {
        let h = PasswordHasherCfg::cheap_for_tests();
        let hash = h.hash("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$v=19$"), "{hash}");
        assert!(h.verify("correct horse battery", &hash));
        assert!(!h.verify("correct horse batter", &hash));
        assert!(!h.verify("anything", "not a phc string"));
        // Two hashes of one password differ (random salt).
        assert_ne!(hash, h.hash("correct horse battery").unwrap());
    }

    #[test]
    fn production_parameters_meet_the_owasp_minimum() {
        let hash = PasswordHasherCfg::owasp().hash("pw").unwrap();
        assert!(hash.contains("m=19456,t=2,p=1"), "{hash}");
    }

    #[test]
    fn totp_accepts_one_step_either_side_only() {
        let secret = new_totp_secret().unwrap();
        let totp = build_totp(secret, "ninad").unwrap();
        let now = 1_800_000_000u64;
        let at = |t: u64| totp.generate(t).to_string();
        let step = now / 30;
        assert_eq!(matching_step(&totp, &at(now), now), Some(step));
        assert_eq!(matching_step(&totp, &at(now - 30), now), Some(step - 1));
        assert_eq!(matching_step(&totp, &at(now + 30), now), Some(step + 1));
        assert_eq!(matching_step(&totp, &at(now - 90), now), None);
        assert_eq!(matching_step(&totp, &at(now + 90), now), None);
        assert_eq!(matching_step(&totp, "abcdef", now), None);
        assert_eq!(matching_step(&totp, "12345", now), None);
    }

    #[test]
    fn totp_uri_names_the_issuer_and_account() {
        let totp = build_totp(new_totp_secret().unwrap(), "ninad").unwrap();
        let uri = totp.to_url().unwrap();
        assert!(uri.starts_with("otpauth://totp/"), "{uri}");
        assert!(uri.contains("issuer=Jarvis"), "{uri}");
        assert!(uri.contains("ninad"), "{uri}");
    }

    #[test]
    fn sealed_secrets_round_trip_and_are_bound_to_key_and_row() {
        let sb = SecretBox::new(b"0123456789abcdef0123456789abcdef").unwrap();
        let sealed = sb.seal(b"secret-bytes", "Ninad").unwrap();
        assert!(sealed.starts_with("v1:"));
        assert!(!sealed.contains("secret-bytes"));
        assert_eq!(sb.open(&sealed, "ninad").unwrap(), b"secret-bytes");
        // Another user's row, another key, tampering and garbage are all rejected.
        assert!(sb.open(&sealed, "other").is_none());
        let other = SecretBox::new(b"ffffffffffffffffffffffffffffffff").unwrap();
        assert!(other.open(&sealed, "ninad").is_none());
        let mut tampered = sealed.clone();
        tampered.pop();
        tampered.push('A');
        assert!(sb.open(&tampered, "ninad").is_none() || tampered == sealed);
        assert!(sb.open("v1:!!!", "ninad").is_none());
        assert!(sb.open("plain", "ninad").is_none());
        // A fresh nonce each time.
        assert_ne!(sealed, sb.seal(b"secret-bytes", "ninad").unwrap());
    }

    #[test]
    fn input_validation() {
        assert!(validate_username("ninad").is_ok());
        assert!(validate_username("n.k_1-a@b").is_ok());
        for bad in ["", "a b", "a:b", &"x".repeat(65), "é"] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
        assert!(validate_new_password("short").is_err());
        assert!(validate_new_password("twelve chars!").is_ok());
        assert!(validate_new_password(&"x".repeat(257)).is_err());
        assert!(validate_secret("short").is_err());
        assert!(validate_secret(&"s".repeat(32)).is_ok());
        assert!(looks_like_code("012345"));
        assert!(!looks_like_code("01234a"));
    }
}
