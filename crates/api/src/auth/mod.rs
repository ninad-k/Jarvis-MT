//! Login: Argon2id password + TOTP, JWT (HS256) session in an HttpOnly cookie (ADR-0011).
//!
//! - [`Accounts`] manages users (the `user` CLI uses it on its own).
//! - [`AuthService`] adds sessions: login with rate limit, lockout and replay protection,
//!   token checks, logout, and the TOTP check that releases the kill switch.
//! - [`http`] holds the routes, the middleware and the cookie handling.
//!
//! Every login failure produces the same response; the reason goes to the audit log only.

pub mod crypto;
pub mod http;
mod store;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::audit::{Audit, AuditKind};
use crate::clock::TimeSource;
use crate::ratelimit::RateLimiter;
use crate::routes::ApiError;
use crate::state::ReleaseGuard;

use self::crypto::{
    PasswordHasherCfg, SecretBox, TOTP_STEP_SECS, build_totp, looks_like_code, matching_step,
    new_totp_secret, validate_new_password, validate_username,
};
pub use self::store::UserSummary;
use self::store::{UserRow, UserStore};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "jarvis_session";
/// Login attempts per minute per client IP (every attempt counts, successful or not).
pub const LOGIN_RATE_LIMIT: usize = 5;
const LOGIN_RATE_WINDOW_MS: i64 = 60_000;
/// Kill-switch release attempts per minute per user.
pub const RELEASE_RATE_LIMIT: usize = 5;
/// Failed attempts that lock an account.
pub const MAX_FAILURES: i64 = 10;
/// How long a locked account refuses every login.
pub const LOCKOUT_MS: i64 = 15 * 60_000;
const MAX_TOKEN_BYTES: usize = 1024;
const MAX_PASSWORD_BYTES: usize = 1024;
/// The message of a refused kill-switch release (wrong, missing, reused or rate-limited
/// code). An expired session says "login required" instead; the UI tells them apart with
/// `GET /api/auth/me`.
pub const CODE_NOT_ACCEPTED: &str = "code not accepted";
/// The one message every login failure carries.
pub const GENERIC_LOGIN_ERROR: &str = "invalid username, password or code";
/// What the audit log shows instead of a name that was typed but belongs to nobody: people
/// type passwords into the user name field, and anyone can type anything.
const NOT_RECORDED: &str = "<not recorded>";

/// An authenticated session, as decoded from a valid token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The token's `jti`: also the rate-limit key of `/api/confirm`.
    pub id: String,
    pub username: String,
    /// Unix seconds.
    pub issued_at: i64,
    /// Unix seconds.
    pub expires_at: i64,
}

impl Session {
    /// RFC 3339 UTC, as returned by `/api/auth/login` and `/api/auth/me`.
    pub fn expires_at_rfc3339(&self) -> String {
        DateTime::<Utc>::from_timestamp(self.expires_at, 0)
            .unwrap_or_default()
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
    iat: u64,
    jti: String,
}

/// What `user add` / `user reset-totp` shows once.
pub struct Enrollment {
    /// `otpauth://totp/...` for an authenticator app.
    pub uri: String,
    /// The same secret as base32 text, for typing in by hand.
    pub secret_base32: String,
}

impl std::fmt::Debug for Enrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Enrollment(<redacted>)")
    }
}

/// User management without sessions.
#[derive(Clone)]
pub struct Accounts {
    store: UserStore,
    sealer: SecretBox,
    hasher: PasswordHasherCfg,
    time: Arc<dyn TimeSource>,
}

impl Accounts {
    pub fn new(
        pool: SqlitePool,
        jwt_secret: &str,
        hasher: PasswordHasherCfg,
        time: Arc<dyn TimeSource>,
    ) -> anyhow::Result<Self> {
        crypto::validate_secret(jwt_secret)?;
        Ok(Self {
            store: UserStore::new(pool),
            sealer: SecretBox::new(jwt_secret.as_bytes())?,
            hasher,
            time,
        })
    }

    fn now_rfc3339(&self) -> String {
        DateTime::<Utc>::from_timestamp_millis(self.time.now_ms())
            .unwrap_or_default()
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn enrollment(&self, username: &str, secret: Vec<u8>) -> anyhow::Result<Enrollment> {
        let totp = build_totp(secret, username)?;
        Ok(Enrollment {
            uri: totp
                .to_url()
                .map_err(|e| anyhow!("building the otpauth URI: {e}"))?,
            secret_base32: totp.secret().to_base32(),
        })
    }

    /// Create a user with a fresh TOTP secret.
    pub async fn add_user(&self, username: &str, password: &str) -> anyhow::Result<Enrollment> {
        validate_username(username)?;
        validate_new_password(password)?;
        let secret = new_totp_secret()?;
        let sealed = self.sealer.seal(&secret, username)?;
        let hasher = self.hasher.clone();
        let password = password.to_owned();
        let hash = tokio::task::spawn_blocking(move || hasher.hash(&password))
            .await
            .context("password hashing task failed")??;
        let created = self.now_rfc3339();
        if !self
            .store
            .insert(username, &hash, &sealed, &created)
            .await?
        {
            bail!("a user named '{username}' already exists");
        }
        self.enrollment(username, secret)
    }

    /// New TOTP secret for an existing user; ends their sessions and clears a lockout.
    pub async fn reset_totp(&self, username: &str) -> anyhow::Result<Enrollment> {
        let user = self
            .store
            .find(username)
            .await?
            .ok_or_else(|| anyhow!("no user named '{username}'"))?;
        let secret = new_totp_secret()?;
        let sealed = self.sealer.seal(&secret, &user.username)?;
        if !self
            .store
            .reset_totp(&user.username, &sealed, self.time.now_ms().div_euclid(1000))
            .await?
        {
            bail!("no user named '{username}'");
        }
        self.enrollment(&user.username, secret)
    }

    pub async fn list_users(&self) -> anyhow::Result<Vec<UserSummary>> {
        self.store.list().await
    }

    /// Delete a user; their sessions stop working at once.
    pub async fn remove_user(&self, username: &str) -> anyhow::Result<bool> {
        self.store.remove(username).await
    }

    pub async fn user_count(&self) -> anyhow::Result<i64> {
        self.store.count().await
    }
}

/// Everything [`AuthService`] is built from.
pub struct AuthParts {
    /// `JARVIS_JWT_SECRET`.
    pub secret: String,
    pub session_hours: u64,
    pub cookie_secure: bool,
    pub time: Arc<dyn TimeSource>,
    pub audit: Audit,
    pub hasher: PasswordHasherCfg,
}

/// Sessions and login. Cheap to share behind an `Arc`.
pub struct AuthService {
    accounts: Accounts,
    audit: Audit,
    time: Arc<dyn TimeSource>,
    encoding: EncodingKey,
    decoding: DecodingKey,
    ttl_secs: i64,
    cookie_secure: bool,
    login_limiter: RateLimiter,
    release_limiter: RateLimiter,
    /// One audit row per client per minute for refused (rate limited) logins, so a flood of
    /// requests cannot grow the append-only audit table without bound.
    rate_audit_limiter: RateLimiter,
    /// `jti` -> expiry (unix seconds) of sessions ended by logout. Mirrors the
    /// `revoked_sessions` table so it survives a restart.
    revoked: RwLock<HashMap<String, i64>>,
    /// Hash verified when the user does not exist, so timing does not reveal it.
    dummy_hash: String,
}

/// A successful login: the cookie value and the session it carries.
#[derive(Debug)]
pub struct LoginOutcome {
    pub token: String,
    pub session: Session,
}

impl AuthService {
    /// Refuses a missing or short secret.
    pub async fn new(parts: AuthParts) -> anyhow::Result<Arc<Self>> {
        crypto::validate_secret(&parts.secret)?;
        let accounts = Accounts::new(
            parts.audit.pool().clone(),
            &parts.secret,
            parts.hasher.clone(),
            parts.time.clone(),
        )?;
        let now_secs = parts.time.now_ms() / 1000;
        let revoked = accounts
            .store
            .load_revoked(now_secs)
            .await?
            .into_iter()
            .collect();
        let dummy = parts.hasher.clone();
        let dummy_hash = tokio::task::spawn_blocking(move || dummy.hash("not-a-real-password"))
            .await
            .context("hashing task failed")??;
        let ttl_secs = i64::try_from(parts.session_hours.saturating_mul(3600)).unwrap_or(i64::MAX);
        Ok(Arc::new(Self {
            accounts,
            audit: parts.audit,
            time: parts.time,
            encoding: EncodingKey::from_secret(parts.secret.as_bytes()),
            decoding: DecodingKey::from_secret(parts.secret.as_bytes()),
            ttl_secs,
            cookie_secure: parts.cookie_secure,
            login_limiter: RateLimiter::new(LOGIN_RATE_LIMIT, LOGIN_RATE_WINDOW_MS),
            release_limiter: RateLimiter::new(RELEASE_RATE_LIMIT, LOGIN_RATE_WINDOW_MS),
            rate_audit_limiter: RateLimiter::new(1, LOGIN_RATE_WINDOW_MS),
            revoked: RwLock::new(revoked),
            dummy_hash,
        }))
    }

    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }
    pub fn cookie_secure(&self) -> bool {
        self.cookie_secure
    }
    pub fn session_ttl_secs(&self) -> i64 {
        self.ttl_secs
    }
    fn now_secs(&self) -> i64 {
        self.time.now_ms().div_euclid(1000)
    }

    async fn audit(&self, kind: AuditKind, payload: serde_json::Value) {
        self.audit.record(None, kind, payload).await;
    }

    // ------------------------------------------------------------------ login

    /// Password + TOTP login. Every failure is the same `401`; the real reason is audited.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        code: &str,
        ip: &str,
    ) -> Result<LoginOutcome, ApiError> {
        let now_ms = self.time.now_ms();
        if let Err(retry_after_secs) = self.login_limiter.check(ip, now_ms) {
            if self.rate_audit_limiter.check(ip, now_ms).is_ok() {
                self.audit(
                    AuditKind::LoginFailed,
                    json!({ "username": NOT_RECORDED, "ip": ip, "reason": "rate_limited" }),
                )
                .await;
            }
            return Err(ApiError::RateLimited { retry_after_secs });
        }
        if password.len() > MAX_PASSWORD_BYTES || username.len() > 64 {
            return Err(self.failed(None, NOT_RECORDED, ip, "malformed").await);
        }

        let user = match self.accounts.store.find(username).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("login: reading the user failed: {e:#}");
                return Err(ApiError::Internal);
            }
        };
        let Some(user) = user else {
            self.verify_dummy(password).await;
            return Err(self.failed(None, NOT_RECORDED, ip, "unknown_user").await);
        };
        // From here on the name is a real account's, as stored (not as typed).
        let shown = user.username.clone();
        if user.locked_until.is_some_and(|until| until > now_ms) {
            self.verify_dummy(password).await;
            return Err(self.failed(None, &shown, ip, "locked").await);
        }

        // Both factors are always evaluated, so the failure does not tell which was wrong.
        let password_ok = self.verify_password(password, &user.password_hash).await;
        let step = match self.totp_step(&user, code) {
            Ok(step) => step,
            Err(reason) => {
                tracing::error!(user = %user.username, "{reason}");
                None
            }
        };
        let (Some(step), true) = (step, password_ok) else {
            let reason = if password_ok {
                "bad_code"
            } else {
                "bad_password"
            };
            return Err(self.failed(Some(&user), &shown, ip, reason).await);
        };
        match self.accounts.store.claim_step(user.id, to_i64(step)).await {
            Ok(true) => {}
            Ok(false) => return Err(self.failed(Some(&user), &shown, ip, "code_reused").await),
            Err(e) => {
                tracing::error!("login: recording the TOTP step failed: {e:#}");
                return Err(ApiError::Internal);
            }
        }

        let now = self.accounts.now_rfc3339();
        if let Err(e) = self.accounts.store.record_success(user.id, &now).await {
            tracing::error!("login: recording the login failed: {e:#}");
            return Err(ApiError::Internal);
        }
        let outcome = self.issue(&user.username)?;
        self.audit(
            AuditKind::Login,
            json!({ "username": user.username, "ip": ip, "jti": outcome.session.id }),
        )
        .await;
        Ok(outcome)
    }

    fn issue(&self, username: &str) -> Result<LoginOutcome, ApiError> {
        let iat = self.now_secs();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            username: username.to_owned(),
            issued_at: iat,
            expires_at: iat.saturating_add(self.ttl_secs),
        };
        let claims = Claims {
            sub: session.username.clone(),
            exp: u64::try_from(session.expires_at).unwrap_or_default(),
            iat: u64::try_from(iat).unwrap_or_default(),
            jti: session.id.clone(),
        };
        let token = self.sign(&claims).map_err(|e| {
            tracing::error!("signing the session token failed: {e}");
            ApiError::Internal
        })?;
        Ok(LoginOutcome { token, session })
    }

    fn sign(&self, claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
        jsonwebtoken::encode(&Header::new(Algorithm::HS256), claims, &self.encoding)
    }

    /// Count the failure (when it belongs to a real, unlocked account), audit it, and build
    /// the one response every failure gets.
    async fn failed(
        &self,
        counted_for: Option<&UserRow>,
        shown: &str,
        ip: &str,
        reason: &str,
    ) -> ApiError {
        let mut locked_now = false;
        if let Some(user) = counted_for {
            let until = self.time.now_ms().saturating_add(LOCKOUT_MS);
            match self
                .accounts
                .store
                .record_failure(user.id, MAX_FAILURES, until)
                .await
            {
                Ok(locked) => locked_now = locked,
                Err(e) => tracing::error!("recording a failed login failed: {e:#}"),
            }
        }
        self.audit(
            AuditKind::LoginFailed,
            json!({ "username": shown, "ip": ip, "reason": reason, "locked_now": locked_now }),
        )
        .await;
        ApiError::Unauthorized(GENERIC_LOGIN_ERROR.to_owned())
    }

    async fn verify_password(&self, password: &str, hash: &str) -> bool {
        let hasher = self.accounts.hasher.clone();
        let (password, hash) = (password.to_owned(), hash.to_owned());
        tokio::task::spawn_blocking(move || hasher.verify(&password, &hash))
            .await
            .unwrap_or(false)
    }

    async fn verify_dummy(&self, password: &str) {
        let _ = self.verify_password(password, &self.dummy_hash).await;
    }

    /// The step whose code matches. `Err` when the stored secret cannot be read (the JWT
    /// secret changed since enrolment): the user needs `user reset-totp`.
    fn totp_step(&self, user: &UserRow, code: &str) -> Result<Option<u64>, String> {
        let secret = self
            .accounts
            .sealer
            .open(&user.totp_secret, &user.username)
            .ok_or_else(|| {
                "the stored TOTP secret cannot be decrypted (was JARVIS_JWT_SECRET changed?). \
                 Run `jarvis-api user reset-totp <name>`"
                    .to_owned()
            })?;
        let totp = build_totp(secret, &user.username).map_err(|e| e.to_string())?;
        if !looks_like_code(code) {
            return Ok(None);
        }
        let now = u64::try_from(self.now_secs()).unwrap_or_default();
        Ok(matching_step(&totp, code, now))
    }

    // --------------------------------------------------------------- sessions

    /// The session a cookie value stands for, if it is genuine, unexpired, not logged out and
    /// the user still exists with no later TOTP reset.
    pub async fn authenticate(&self, token: &str) -> Option<Session> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let claims = self.decode(token)?;
        let (exp, iat) = (
            i64::try_from(claims.exp).ok()?,
            i64::try_from(claims.iat).ok()?,
        );
        if exp <= self.now_secs() {
            return None;
        }
        if self
            .revoked
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&claims.jti)
        {
            return None;
        }
        match self.accounts.store.token_floor(&claims.sub).await {
            Ok(Some(floor)) if iat >= floor => Some(Session {
                id: claims.jti,
                username: claims.sub,
                issued_at: iat,
                expires_at: exp,
            }),
            Ok(_) => None,
            Err(e) => {
                tracing::error!("checking a session failed: {e:#}");
                None
            }
        }
    }

    fn decode(&self, token: &str) -> Option<Claims> {
        let mut validation = Validation::new(Algorithm::HS256);
        // Expiry is checked against our own clock in `authenticate` (testable, no leeway).
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        validation.required_spec_claims = ["exp", "sub"].into_iter().map(String::from).collect();
        jsonwebtoken::decode::<Claims>(token, &self.decoding, &validation)
            .ok()
            .map(|data| data.claims)
    }

    /// Is this session still good? Used by long-lived connections (WebSocket).
    pub async fn is_current(&self, session: &Session) -> bool {
        if session.expires_at <= self.now_secs() {
            return false;
        }
        if self
            .revoked
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&session.id)
        {
            return false;
        }
        matches!(
            self.accounts.store.token_floor(&session.username).await,
            Ok(Some(floor)) if session.issued_at >= floor
        )
    }

    /// End a session for good (until its token would have expired anyway).
    pub async fn logout(&self, session: &Session) {
        self.revoked
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session.id.clone(), session.expires_at);
        if let Err(e) = self
            .accounts
            .store
            .revoke(&session.id, session.expires_at)
            .await
        {
            // Still revoked in memory until restart.
            tracing::error!("persisting a logout failed: {e:#}");
        }
        self.audit(
            AuditKind::Login,
            json!({ "username": session.username, "jti": session.id, "event": "logout" }),
        )
        .await;
    }

    // ----------------------------------------------------------- kill switch

    /// The TOTP check that releasing the kill switch needs. A code works once: the step is
    /// recorded like at login, so a code just used to log in must be followed by the next.
    pub async fn verify_release_totp(&self, session: &Session, code: &str) -> Result<(), String> {
        let now_ms = self.time.now_ms();
        if self
            .release_limiter
            .check(&session.username, now_ms)
            .is_err()
        {
            tracing::warn!(user = %session.username, "kill-switch release attempts rate limited");
            return Err(CODE_NOT_ACCEPTED.to_owned());
        }
        let user = match self.accounts.store.find(&session.username).await {
            Ok(Some(u)) => u,
            Ok(None) => return Err(CODE_NOT_ACCEPTED.to_owned()),
            Err(e) => {
                tracing::error!("kill-switch release: reading the user failed: {e:#}");
                return Err(CODE_NOT_ACCEPTED.to_owned());
            }
        };
        let refuse = |reason: &'static str| async move {
            self.audit(
                AuditKind::KillSwitch,
                json!({ "release_refused": reason, "username": session.username, "jti": session.id }),
            )
            .await;
            Err(CODE_NOT_ACCEPTED.to_owned())
        };
        if user.locked_until.is_some_and(|until| until > now_ms) {
            return refuse("locked").await;
        }
        let step = self.totp_step(&user, code).ok().flatten();
        let accepted = match step {
            Some(step) => self
                .accounts
                .store
                .claim_step(user.id, to_i64(step))
                .await
                .unwrap_or(false),
            None => false,
        };
        if accepted {
            return Ok(());
        }
        let until = now_ms.saturating_add(LOCKOUT_MS);
        if let Err(e) = self
            .accounts
            .store
            .record_failure(user.id, MAX_FAILURES, until)
            .await
        {
            tracing::error!("recording a failed release failed: {e:#}");
        }
        refuse(if step.is_some() {
            "code_reused"
        } else {
            "bad_code"
        })
        .await
    }

    /// Forget idle rate-limit keys and expired revocations.
    pub async fn purge(&self) {
        let now_ms = self.time.now_ms();
        self.login_limiter.purge(now_ms);
        self.release_limiter.purge(now_ms);
        self.rate_audit_limiter.purge(now_ms);
        let now_secs = now_ms.div_euclid(1000);
        let expired = {
            let mut revoked = self.revoked.write().unwrap_or_else(|p| p.into_inner());
            let before = revoked.len();
            revoked.retain(|_, exp| *exp > now_secs);
            before != revoked.len()
        };
        if expired && let Err(e) = self.accounts.store.purge_revoked(now_secs).await {
            tracing::warn!("purging revoked sessions failed: {e:#}");
        }
    }
}

/// Replaces the open release guard when login is enabled: a session and a fresh TOTP code.
pub struct AuthReleaseGuard(pub Arc<AuthService>);

#[async_trait]
impl ReleaseGuard for AuthReleaseGuard {
    async fn authorize_release(
        &self,
        session: Option<&Session>,
        totp: Option<&str>,
    ) -> Result<(), String> {
        let session = session.ok_or_else(|| "login required".to_owned())?;
        let code = totp
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| CODE_NOT_ACCEPTED.to_owned())?;
        self.0.verify_release_totp(session, code).await
    }
}

fn to_i64(step: u64) -> i64 {
    i64::try_from(step).unwrap_or(i64::MAX)
}

/// Seconds in one TOTP step; re-exported for tests that move the clock.
pub const TOTP_STEP: u64 = TOTP_STEP_SECS;
