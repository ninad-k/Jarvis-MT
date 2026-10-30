# ADR-0003: Own JSON parser in MQL5

- Status: Accepted
- Date: 2026-10-30

## Context

The bridge protocol is JSON (see [docs/protocol.md](../protocol.md)). MQL5 has no built-in JSON support, and MT5 does not ship a suitable JSON library. The EA must parse every request it receives, reject malformed input without crashing, and build replies with the same shape the Rust side expects.

## Decision

The EA uses our own small module, `mql5/Include/Jarvis/Json.mqh`, of about 300 lines. It has two parts:

- a parser that turns text into a tagged value tree, stored in a flat node pool with no pointers and bounds-checked indexes;
- a string builder for replies.

The parser is small enough to read in full, so every input check is explicit. Malformed input gets an error reply and never crashes the EA. Tests use the same golden JSON fixtures as the Rust side.

## Consequences

- The project carries its own parser, which must be kept correct and tested. The protocol uses only a small subset of JSON, which keeps the parser simple.
- There is no third-party MQL5 dependency to license, audit or update.
- Input validation is visible in one place. The EA rejects bad types, out-of-range values and unknown symbols itself.
- Request size is bounded by the 64 KB ZeroMQ message limit described in the protocol document, which also bounds the work the parser does per request.

## Alternatives considered

- **A community MQL5 JSON library.** We did not find one that is suitably licensed and maintained for this use. Adopting one would also add a dependency that we would have to vet for safety.
- **A non-JSON wire format**, such as `key=value` lines or CSV. It is easier to parse in MQL5, but the Rust side and the HTTP layer would need custom code, and the golden fixtures would be less readable.
- **Parsing with string functions only.** It is fragile for nested objects and arrays, and it makes input validation harder to read and test.
