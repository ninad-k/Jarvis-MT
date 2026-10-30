-- Login accounts (ADR-0011). One row per user; the database file holds the Argon2id
-- password hash and the encrypted TOTP secret, so it must be readable only by the account
-- that runs Jarvis.
CREATE TABLE users (
    id                INTEGER PRIMARY KEY,
    -- Compared case-insensitively; the CLI stores the name as typed.
    username          TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- PHC string: $argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>
    password_hash     TEXT NOT NULL,
    -- "v1:" + base64(nonce || ChaCha20-Poly1305 ciphertext), key derived from
    -- JARVIS_JWT_SECRET with HKDF-SHA256. Never the plain secret.
    totp_secret       TEXT NOT NULL,
    -- Time step (unix seconds / 30) of the last accepted code; a code is only accepted for a
    -- later step, so no code works twice.
    totp_last_step    INTEGER,
    created_at        TEXT NOT NULL,
    last_login_at     TEXT,
    failed_attempts   INTEGER NOT NULL DEFAULT 0,
    -- Unix ms until which logins are refused; NULL when not locked.
    locked_until      INTEGER,
    -- Sessions issued at or before this unix second are rejected (set when the TOTP secret
    -- is reset, so the old sessions die at once).
    tokens_valid_after INTEGER NOT NULL DEFAULT 0
);

-- Sessions ended by logout, kept until their token would have expired anyway. Survives a
-- restart, so a logged-out cookie stays dead.
CREATE TABLE revoked_sessions (
    jti        TEXT PRIMARY KEY,
    expires_at INTEGER NOT NULL
);

CREATE INDEX revoked_sessions_expiry_idx ON revoked_sessions (expires_at);
