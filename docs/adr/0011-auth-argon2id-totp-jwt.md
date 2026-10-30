# ADR-0011: Authentication with Argon2id, TOTP and a JWT session cookie

- Status: Accepted
- Date: 2026-10-30

## Context

From Sprint 2 Jarvis MT can place trades. Until authentication arrives, the API binds to loopback only and needs no login. Phone use (planned for Sprint 5) and any other non-loopback binding would make the API reachable beyond the PC, and anyone who could reach it could then confirm trades. The app is single-tenant, with one trader, so the design can be simple. The login must still resist password guessing and stolen passwords, and it must not expose the session to page scripts.

## Decision

- Passwords are hashed with **Argon2id**. Users are created from the command line (`jarvis-api user add <name>`), which prints a TOTP `otpauth://` URI for an authenticator app.
- Login needs the password **and** a TOTP code. Every login failure returns the same `401 unauthorized` response.
- After login, the API issues a **JWT** signed with HS256. The signing secret comes from the environment variable `JARVIS_JWT_SECRET` and must be at least 32 bytes. It is kept in `.env`, never in config.
- The JWT is stored in the cookie `jarvis_session`, with `HttpOnly; Secure; SameSite=Strict`. Its lifetime is `auth.session_hours` (12 hours by default).
- Rate limits: login is limited to 5 attempts per minute per IP, and confirmation to 10 per minute per session.
- Binding the API to any address other than loopback requires both TLS and auth to be enabled.

Status: planned for Sprint 4.

## Consequences

- A stolen password alone is not enough, because the TOTP code is also required.
- The cookie cannot be read by page scripts, which reduces the damage from a cross-site scripting bug.
- A JWT stays valid until it expires, and the plan does not include a server-side revocation list. The 12-hour lifetime bounds the exposure. Logging out clears the cookie. A revocation list could be added later if needed.
- Losing the TOTP device means the user must be re-enrolled from the command line on the PC.
- The authenticator and the JWT secret are local secrets, so the backup of `.env` matters.

## Alternatives considered

- **Password only.** A password alone is weak protection for an app that can trade.
- **A server-side session table.** It can be revoked instantly, and it is also reasonable. We preferred a signed token with a short lifetime for a single-user app with no session store to manage. This choice can be revisited.
- **Hosted identity (OAuth or OpenID Connect).** It adds an external dependency and a third party to a single-user tool.
- **Passkeys (WebAuthn).** Good for phones, but more complex to set up on a Windows PC and a PWA in the first version.
- **API keys sent in a header.** Keys are easier to leak from a browser than a HttpOnly cookie.
