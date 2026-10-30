# ADR-0016: Authenticated bridge protocol (HMAC-SHA256 over two frames)

- Status: Accepted
- Date: 2026-10-30

## Context

The EA binds `tcp://127.0.0.1:5555` and, until protocol 1, answered any local process (ADR-0002). The loopback bind keeps the network out, but not other programs on the same PC, and not another Windows user on a shared PC or VPS. Such a program could send `order_send` directly to the EA and skip the confirmation, the limits, the kill switch and the audit log. The EA's own checks (field validation, `InpAllowReal`, Algo Trading) still applied, so the damage was limited to a demo account or to what those checks allow, but the rest of the safety design assumed that only the API talks to the EA. This is finding BR-1 in the [security review](../security/audit-2026-10-30.md).

The reverse also mattered: a process that grabbed the port before the EA, or answered in its place, could feed the API fake prices, positions and trade results.

Constraints: the pure-Rust `zeromq` crate used by the API has no CURVE support; the EA must stay non-blocking and answer every request exactly once (REP is lock-step); MQL5 has `CryptEncode(CRYPT_HASH_SHA256, ...)` and nothing else cryptographic that does not need a DLL; the committed ZeroMQ DLLs must not become more complicated.

## Decision

Protocol version 2 authenticates every message in both directions with a shared secret.

- **Key.** 32 random bytes from a CSPRNG, as 64 hex characters in a text file. `scripts/install-ea.ps1` creates it at `<MT5 data folder>\MQL5\Files\JarvisMT\bridge.key` if it does not exist (`RandomNumberGenerator`, exclusive create, never overwritten, never printed; `-RotateKey` replaces it on request). That folder is the EA's file sandbox and sits in the user's profile, so its ACL already restricts it to that user. The EA reads it with `FileOpen` in `OnInit` and does not start without a valid key. The API reads `[bridge] key_file` or `JARVIS_BRIDGE_KEY` and does not start without one. Neither side logs it.
- **Frames.** Each message is a ZeroMQ multipart message of exactly two frames: frame 1 is the lowercase hex of `HMAC-SHA256(key, frame 2)`, frame 2 is the JSON payload byte for byte as sent. The MAC is over the raw bytes, so there is no JSON canonicalisation. A message with any other frame count is refused.
- **Freshness and replay.** A request carries `"ts"` (Unix milliseconds, client clock). The EA refuses a request whose `ts` is more than 30 s from its own clock (`TimeGMT()` plus `GetTickCount64()` for the milliseconds) or whose `id` it accepted in the last 60 s. It keeps those ids in a table of at most 4096 entries, expired on a monotonic clock; when the table is full it refuses new requests until entries expire.
- **Refusal.** The EA answers a refused request exactly once, with a signed reply, retcode `-4` and the fixed text `authentication failed`. It checks all of this before it parses the arguments, consults the write cache or touches `CTrade`. The reply to an unauthenticated request has an empty `id`, so the signed reply cannot be used to obtain a signature over text the attacker chose.
- **Replies.** Signed the same way, with an added `"ts"`. The client checks the MAC of every reply in constant time, and the reply `id`; on failure it returns `BridgeError::Protocol("reply authentication failed")` and drops the socket.
- **Version.** `protocol_version` is 2 and the EA is 0.3.0. `BridgeClient::ping` refuses an EA with another version; an old EA answers a two-frame request with one plain frame, which the client reports as such.
- **Implementation.** MQL5 HMAC-SHA256 on top of `CryptEncode` (RFC 2104, block size 64), hex, a constant-time comparison. Rust uses the RustCrypto `hmac` and `sha2` crates (already in the dependency tree through `hkdf` and `sha2`). A golden file (`tests/fixtures/auth.golden.json`) and the RFC 4231 vectors are checked by both implementations; a Rust test fails if the MQL5 self-test and the golden file disagree.

## Consequences

- A program that does not hold the key cannot execute or even read anything through the bridge, and cannot impersonate the EA. This closes BR-1 for other Windows users and for any process that cannot read the key file.
- A program that can read `bridge.key` (the same Windows account, an administrator) is not stopped; SECURITY.md already excludes a compromised machine or account. The messages are not encrypted: an administrator who can capture loopback traffic can read them. A captured request cannot be replayed, because the EA accepted its `id` before the capture existed and refuses it for 60 s, and its `ts` is stale after 30 s.
- Every request needs a new `id` and a correct clock. A PC clock off by more than 30 s makes every request fail with `-4`; the Experts journal says why. Both sides normally run on the same PC.
- The EA's write reply cache (ADR-0012) cannot be reached any more, because a repeated `id` is refused first. It stays as defence in depth. The old statement that sending the same `id` again returns the cached reply no longer holds; after a timeout the caller reconciles by reading state (ADR-0013), as before.
- A peer that floods the socket costs the EA one HMAC and one reply per message. There is no rate limit; the flood can delay legitimate requests but cannot execute anything. A keyed client that sends more than 4096 requests in a minute is refused until entries expire, which no real client does.
- Rotating the key needs the EA to be re-attached and the API restarted. The EA reads the key only at start.
- Protocol 2 is not compatible with protocol 1, so the EA and the API must be updated together. The mock bridge speaks protocol 2 with a random key per instance, so all API tests exercise the real authenticated path.
- The EA loads the key from a file in `MQL5\Files`, which the MT5 terminal and every other EA of the same terminal can read. The terminal is already trusted with the broker login.

## Alternatives considered

- **ZeroMQ CURVE.** It would also encrypt, but the `zeromq` crate has no CURVE support and the committed libzmq no longer includes libsodium. Dropping the Rust crate for a binding to libzmq would be a larger change than the threat justifies.
- **A MAC over selected fields (`id`, `cmd`, canonical arguments), as first proposed in the review.** It needs a canonical JSON form in two languages, and a bug there silently weakens the check. A MAC over the exact bytes cannot disagree about the representation.
- **A single-frame message with the MAC inside the JSON.** The MAC would have to cover the JSON without itself, which brings back canonicalisation or a fixed field order.
- **A per-request nonce chosen by the EA (challenge and response).** It removes the clock dependence, but REP/REQ lock-step has no room for a second round trip without blocking the EA or changing the socket types.
- **Named pipes or a Unix-style socket with an ACL.** MQL5 can open named pipes only through file functions with limited semantics, and ZeroMQ does not use them on Windows.
- **A `ts` window of 5 s.** It shrinks the replay window but makes the bridge fail on a small clock step. The replay table, not the window, is what stops a replay; 30 s only bounds how long the table has to remember.
