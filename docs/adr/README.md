# Architecture decision records

An architecture decision record (ADR) describes one decision, the context that led to it, and its consequences. We use ADRs for decisions that affect the architecture, the safety rules in [SPEC.md](../../SPEC.md), the wire and HTTP contracts, security boundaries, and dependencies.

The format is a short [MADR](https://adr.github.io/madr/)-style record. Each ADR is one Markdown file in this directory. Files are numbered in the order they are written and never renumbered.

## Index

| ADR | Title | Status | Date |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted | 2026-10-30 |
| [0002](0002-zeromq-req-rep-bridge.md) | ZeroMQ REQ/REP bridge with the EA as a loopback REP server | Accepted | 2026-10-30 |
| [0003](0003-own-json-parser-in-mql5.md) | Own JSON parser in MQL5 | Accepted | 2026-10-30 |
| [0004](0004-commit-zeromq-dlls-with-checksums.md) | Commit the ZeroMQ DLLs with SHA-256 checksums | Accepted | 2026-10-30 |
| [0005](0005-llm-read-and-propose-tools-only.md) | The LLM gets read tools and propose tools only | Accepted | 2026-10-30 |
| [0006](0006-deterministic-intent-parser.md) | Deterministic intent parser for English and Hinglish | Accepted | 2026-10-30 |
| [0007](0007-voice-token-readback-timing.md) | Voice confirmation window starts after read-back | Accepted | 2026-10-30 |
| [0008](0008-batch-second-confirmation.md) | Batch actions need a second confirmation with a fresh token | Accepted | 2026-10-30 |
| [0009](0009-est-pnl-definition.md) | Definition of estimated P/L in previews | Accepted | 2026-10-30 |
| [0010](0010-rust-computes-money-totals.md) | Rust computes every money total | Accepted | 2026-10-30 |
| [0011](0011-auth-argon2id-totp-jwt.md) | Authentication with Argon2id, TOTP and a JWT session cookie | Accepted | 2026-10-30 |
| [0012](0012-ea-write-safety.md) | Write safety in the EA | Accepted | 2026-10-30 |
| [0013](0013-unknown-outcome-for-timed-out-writes.md) | Timed-out writes have an unknown outcome and are never retried | Accepted | 2026-10-30 |
| [0014](0014-local-first-models.md) | Local-first models, with cloud LLMs as an option | Accepted | 2026-10-30 |
| [0015](0015-desktop-tauri-and-pwa.md) | Desktop app with Tauri 2, and a PWA for phones | Accepted | 2026-10-30 |
| [0016](0016-authenticated-bridge-protocol.md) | Authenticated bridge protocol (HMAC-SHA256 over two frames) | Accepted | 2026-10-30 |
| [0017](0017-personas.md) | Personas change how Arc MT sounds, not what it may do | Accepted | 2026-10-30 |
| [0018](0018-broker-symbol-mapping.md) | Broker symbol mapping with canonical names and per-broker layered maps | Accepted | 2026-10-30 |

ADRs 0005 to 0015 record decisions made for Sprints 2 to 5. Their features are planned and not yet implemented.

## When to write one

Write an ADR when a change:

- alters a core rule in [SPEC.md](../../SPEC.md);
- changes the bridge wire protocol, the HTTP API, or the EA inputs;
- touches the action gate, confirmation tokens, the kill switch, authentication, or the code that writes to MT5. These changes must have one, as stated in [CONTRIBUTING.md](../../CONTRIBUTING.md);
- adds a dependency, vendored code or a committed binary;
- would otherwise need the same explanation twice.

## How to write one

1. Copy the template below to `docs/adr/NNNN-short-title.md`, using the next free number.
2. Set the status to `Proposed` and open a pull request.
3. A maintainer sets the status to `Accepted` when the pull request merges, or to `Rejected`.
4. Do not rewrite an accepted decision. If it changes, write a new ADR that supersedes it, and change the old ADR's status to `Superseded by ADR-NNNN`. Fixing typos and broken links is fine.

Status values: `Proposed`, `Accepted`, `Rejected`, `Deprecated`, `Superseded by ADR-NNNN`.

## Template

```markdown
# ADR-NNNN: Short title

- Status: Proposed
- Date: YYYY-MM-DD

## Context

The problem, the forces at work, and the facts that shaped the decision.

## Decision

What we decided, in full sentences.

## Consequences

What becomes easier or harder, including costs, risks and follow-up work.

## Alternatives considered

Other options we looked at, and why we did not choose them.
```
