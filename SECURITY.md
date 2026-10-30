# Security policy

Arc MT sits between a language model and a trading terminal, so security reports are taken seriously. Thank you for helping to keep users' accounts safe.

## Supported versions

Arc MT is pre-1.0. Only the latest release gets security fixes.

| Version | Supported |
|---|---|
| Latest `0.x` release | Yes |
| Older `0.x` releases | No. Please upgrade. |
| `main` (unreleased) | Fixes land here first |

## Reporting a vulnerability

**Do not open a public issue, discussion or pull request for a vulnerability.**

Use GitHub's private vulnerability reporting:

> [Report a vulnerability](https://github.com/ninad-k/Jarvis-MT/security/advisories/new)

Include:

- what you found and why it matters (for example, "a request without a token reaches the bridge");
- the affected version or commit;
- steps to reproduce, or a proof of concept (a demo account is enough; please never test on a real account);
- any suggested fix.

Redact API keys, tokens and account numbers from everything you send.

### What to expect

These are targets for a single-maintainer project, not a contract.

| Step | Target |
|---|---|
| Acknowledge your report | 3 days |
| Initial assessment (accepted, needs more information, or out of scope) | 7 days |
| Fix or mitigation for a critical or high issue | 30 days |
| Fix for a medium or low issue | Next regular release |
| Public advisory | After a fix is released, credited to you unless you prefer to stay anonymous |

If you do not hear back within the acknowledgement window, please add a comment on the advisory.

## Scope

In scope:

- **Action gate bypass.** Any way to make a write command reach MT5 without a valid, unexpired, one-time confirmation token; a way to reuse, forge or extend a token; a way around the limits (`max_lot`, `max_positions`, `max_daily_loss`, `allowed_symbols`, `trading_hours`) or around the batch double confirmation.
- **Kill switch bypass.** A write that succeeds while the kill switch is on.
- **LLM-driven execution.** Any tool, prompt or input that lets the model (or text it is shown) cause a trade without the user's confirmation.
- **Authentication and sessions.** Login, TOTP, session cookies and JWT handling, rate limiting, access control on any HTTP or WebSocket route.
- **Token and secret handling.** Leaks of API keys, the JWT secret, confirmation tokens, or session cookies, for example into logs, error messages or the browser.
- **Bridge exposure.** The ZeroMQ bridge or the API being reachable from outside the machine, or accepting commands from an unintended peer.
- **Credential leakage.** Account login, name or server reaching the LLM, the browser, logs or the audit log.
- **Demo/real mismatch.** Running in demo mode against a real account without being stopped.
- **Supply chain.** Tampered release artifacts, missing or invalid checksums or build attestations, and vulnerable dependencies that are actually reachable.

Out of scope:

- Bugs or outages in MetaTrader 5, your broker, or your VPS provider.
- Losses caused by settings the user chose, such as generous limits, a real account, or a confirmation the user gave.
- Mistakes by a language model that never reach the confirmation step (a wrong answer is not a vulnerability; a wrong trade without confirmation is).
- Attacks that need an already compromised machine, a stolen unlocked device, or administrator access to the PC that runs Arc MT.
- Denial of service by a client that already holds a valid session.
- Findings from automated scanners without a demonstrated impact, and missing security headers on a loopback-only deployment.
- Issues in third-party services (Anthropic, OpenAI, Ollama) or in the vendored mql-zmq and libzmq libraries that are not specific to how Arc MT uses them. Please report those upstream.

## Safe harbour

If you act in good faith, follow this policy, and stay within these limits, I will not pursue or support legal action against you for your research:

- test only against your own installation and a **demo** account;
- do not access, change or delete data that is not yours, and stop as soon as you have shown the problem;
- do not degrade a service other people use;
- give me reasonable time to fix the issue before you disclose it.

## Security model in brief

The design goal is that a trade cannot happen unless the user explicitly confirms it, even if the language model is wrong or manipulated. The full rules are in [SPEC.md](SPEC.md).

- **The model cannot trade.** It can call read tools and `propose_*` tools. A proposal produces a preview and a one-time token; it never sends an order.
- **Confirmation gate.** Only a confirm request carrying a valid token (a button press or a spoken "confirm") sends a write command. Tokens last 15 seconds and work once. Batch actions need a second confirmation that repeats the count.
- **Limits first.** Lot size, open positions, daily loss, allowed symbols and trading hours are checked before a preview is issued. An empty symbol allow-list blocks everything.
- **Kill switch.** One switch disables all write endpoints, including a confirmation already in flight. It stays engaged across a restart, and releasing it needs a login session and a fresh authenticator code.
- **Demo by default.** Live trading needs an explicit setting and a UI banner. The backend refuses to start in demo mode when the account reports real, and refuses chat if the account switches while it runs. The EA refuses writes on a real account unless it was started with `InpAllowReal = true`.
- **Authenticated loopback bridge.** The EA listens on `127.0.0.1` only, and every request and reply is signed with HMAC-SHA256 under a per-install key, with timestamp and replay checks, so other local processes cannot send it orders. The API binds to loopback only until authentication is enabled; exposing it beyond the machine requires TLS and login.
- **Authentication.** Argon2id password hashes, TOTP as a second factor, and a signed session in an `HttpOnly`, `Secure`, `SameSite=Strict` cookie. Login attempts and confirmations are rate limited. Login is on by default. Browser requests that change state are refused unless they come from the server's own origin, and with login switched off the API only answers requests addressed to a loopback host name (a guard against DNS rebinding).
- **Credentials stay in MT5.** Account login, name and server are never sent to the language model or the browser. API keys live in `.env`, not in the config file or the repository.
- **Audit trail.** Every step (utterance, intent, preview, confirmation, bridge request, retcode) is written to an append-only audit log.
- **Supply chain.** Dependencies are checked with `cargo deny`, `cargo audit` and `pnpm audit`; secrets with gitleaks; code with semgrep and CodeQL. Release artifacts ship with `SHA256SUMS` and a build provenance attestation.

### Verifying a download

Check the hash against `SHA256SUMS` from the same release, and verify the attestation with the GitHub CLI:

```powershell
Get-FileHash .\jarvis-api-v0.1.0-windows-x64.zip -Algorithm SHA256
gh attestation verify .\jarvis-api-v0.1.0-windows-x64.zip --repo ninad-k/Jarvis-MT
```

The Windows desktop installer is not code-signed yet, so SmartScreen may warn when you run it. The attestation is how you confirm it came from this repository's release workflow.
