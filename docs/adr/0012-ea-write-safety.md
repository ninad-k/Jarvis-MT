# ADR-0012: Write safety in the EA

- Status: Accepted
- Date: 2026-10-30

## Context

SPEC rule 7 makes demo the default, and the backend refuses to start in demo mode against a real account. Those checks run in the Rust backend. The EA is the last component before MT5, so it needs its own guard. A bug or misconfiguration in the backend should not be able to trade on a real account. A resent request should also not place the same order twice.

## Decision

- The EA refuses every write command on a real account unless its input `InpAllowReal` is set to `true`. The default is `false`. The check runs in the EA, separately from the backend's checks.
- The EA keeps the last 64 write replies in memory, keyed by request `id`. A request with an id it has already answered gets the stored reply and is not executed again.
- The EA trades only when it receives a request. There is no trading logic in `OnTick` or `OnTimer`.
- Every write command is validated before use, as for read commands (ADR-0002).

Status: planned for Sprint 2. The read-only EA in Sprint 1 has no write commands.

## Consequences

- A real account needs two independent decisions: the backend's live mode and the EA input. Setting only one of them does not allow trading.
- Replies are cached in memory, so the cache is empty after the EA restarts. Across a restart, duplicate protection depends on the backend's rule that a sent write is never retried (ADR-0013).
- The EA's behaviour is simple to read: it does nothing unless asked, which makes review and testing easier.

## Alternatives considered

- **Backend checks only.** A single guard is easier to bypass by mistake, so we added the EA guard as defence in depth.
- **A demo-only EA build.** Two builds would need to be kept in step, and users would need to install the right one.
- **Trading logic in the EA on a timer**, for example to manage stops. This breaks SPEC rule 1 and the rule that the EA never acts without a request.
