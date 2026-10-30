//! SQL for the `users` and `revoked_sessions` tables (migration 0002). Every query is
//! parameterised.

use anyhow::Context;
use sqlx::{Row, SqlitePool};

/// A user as stored. `password_hash` and `totp_secret` never leave the `auth` module.
#[derive(Clone)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub totp_secret: String,
    pub locked_until: Option<i64>,
}

impl std::fmt::Debug for UserRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserRow")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password_hash", &"<redacted>")
            .field("totp_secret", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// What `user list` shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub username: String,
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub locked_until: Option<i64>,
}

#[derive(Clone)]
pub struct UserStore {
    pool: SqlitePool,
}

impl UserStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find(&self, username: &str) -> anyhow::Result<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT id, username, password_hash, totp_secret, locked_until \
             FROM users WHERE username = ?1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .context("reading a user")?;
        row.map(|r| {
            Ok(UserRow {
                id: r.try_get("id")?,
                username: r.try_get("username")?,
                password_hash: r.try_get("password_hash")?,
                totp_secret: r.try_get("totp_secret")?,
                locked_until: r.try_get("locked_until")?,
            })
        })
        .transpose()
    }

    pub async fn count(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM users")
            .fetch_one(&self.pool)
            .await
            .context("counting users")?;
        Ok(row.try_get("n")?)
    }

    /// `Ok(false)` when the name is taken (case-insensitively).
    pub async fn insert(
        &self,
        username: &str,
        password_hash: &str,
        totp_secret: &str,
        created_at: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "INSERT INTO users (username, password_hash, totp_secret, created_at) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(username) DO NOTHING",
        )
        .bind(username)
        .bind(password_hash)
        .bind(totp_secret)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .context("creating a user")?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<UserSummary>> {
        let rows = sqlx::query(
            "SELECT username, created_at, last_login_at, locked_until FROM users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .context("listing users")?;
        rows.into_iter()
            .map(|r| {
                Ok(UserSummary {
                    username: r.try_get("username")?,
                    created_at: r.try_get("created_at")?,
                    last_login_at: r.try_get("last_login_at")?,
                    locked_until: r.try_get("locked_until")?,
                })
            })
            .collect()
    }

    pub async fn remove(&self, username: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM users WHERE username = ?1")
            .bind(username)
            .execute(&self.pool)
            .await
            .context("removing a user")?;
        Ok(result.rows_affected() == 1)
    }

    /// Replace the TOTP secret, forget the last used step, clear a lockout and end every
    /// session issued up to `now_secs`.
    pub async fn reset_totp(
        &self,
        username: &str,
        sealed_secret: &str,
        now_secs: i64,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE users SET totp_secret = ?2, totp_last_step = NULL, failed_attempts = 0, \
             locked_until = NULL, tokens_valid_after = ?3 WHERE username = ?1",
        )
        .bind(username)
        .bind(sealed_secret)
        .bind(now_secs)
        .execute(&self.pool)
        .await
        .context("resetting the TOTP secret")?;
        Ok(result.rows_affected() == 1)
    }

    /// Accept time step `step` for this user. True only if it is later than every step
    /// accepted before, decided atomically, so two concurrent logins cannot both use a code.
    pub async fn claim_step(&self, user_id: i64, step: i64) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE users SET totp_last_step = ?2 \
             WHERE id = ?1 AND (totp_last_step IS NULL OR totp_last_step < ?2)",
        )
        .bind(user_id)
        .bind(step)
        .execute(&self.pool)
        .await
        .context("recording the TOTP step")?;
        Ok(result.rows_affected() == 1)
    }

    /// Count a failed attempt. When it reaches `max_failures` the account is locked until
    /// `lock_until_ms` and the counter restarts. Returns true if this attempt locked it.
    pub async fn record_failure(
        &self,
        user_id: i64,
        max_failures: i64,
        lock_until_ms: i64,
    ) -> anyhow::Result<bool> {
        let row = sqlx::query(
            "UPDATE users SET \
               locked_until = CASE WHEN failed_attempts + 1 >= ?2 THEN ?3 ELSE locked_until END, \
               failed_attempts = CASE WHEN failed_attempts + 1 >= ?2 THEN 0 \
                                      ELSE failed_attempts + 1 END \
             WHERE id = ?1 RETURNING failed_attempts",
        )
        .bind(user_id)
        .bind(max_failures)
        .bind(lock_until_ms)
        .fetch_optional(&self.pool)
        .await
        .context("recording a failed login")?;
        Ok(row
            .map(|r| r.try_get::<i64, _>("failed_attempts"))
            .transpose()?
            .is_some_and(|n| n == 0))
    }

    pub async fn record_success(&self, user_id: i64, now_rfc3339: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE users SET failed_attempts = 0, locked_until = NULL, last_login_at = ?2 \
             WHERE id = ?1",
        )
        .bind(user_id)
        .bind(now_rfc3339)
        .execute(&self.pool)
        .await
        .context("recording a login")?;
        Ok(())
    }

    /// `tokens_valid_after` of a user; `None` when the user no longer exists.
    pub async fn token_floor(&self, username: &str) -> anyhow::Result<Option<i64>> {
        let row = sqlx::query("SELECT tokens_valid_after FROM users WHERE username = ?1")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .context("reading the session floor")?;
        Ok(row.map(|r| r.try_get("tokens_valid_after")).transpose()?)
    }

    // ---- revoked sessions ----------------------------------------------------------

    pub async fn revoke(&self, jti: &str, expires_at: i64) -> anyhow::Result<()> {
        sqlx::query("INSERT OR IGNORE INTO revoked_sessions (jti, expires_at) VALUES (?1, ?2)")
            .bind(jti)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .context("revoking a session")?;
        Ok(())
    }

    /// Revoked sessions that have not expired yet, as `(jti, expires_at)`.
    pub async fn load_revoked(&self, now_secs: i64) -> anyhow::Result<Vec<(String, i64)>> {
        let rows =
            sqlx::query("SELECT jti, expires_at FROM revoked_sessions WHERE expires_at > ?1")
                .bind(now_secs)
                .fetch_all(&self.pool)
                .await
                .context("loading revoked sessions")?;
        rows.into_iter()
            .map(|r| Ok((r.try_get("jti")?, r.try_get("expires_at")?)))
            .collect()
    }

    pub async fn purge_revoked(&self, now_secs: i64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM revoked_sessions WHERE expires_at <= ?1")
            .bind(now_secs)
            .execute(&self.pool)
            .await
            .context("purging revoked sessions")?;
        Ok(())
    }
}
