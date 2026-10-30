# ADR-0006: Deterministic intent parser for English and Hinglish

- Status: Accepted
- Date: 2026-10-30

## Context

The model can call `propose_*` tools (ADR-0005), so it decides what the user asked for. Two things can go wrong. The model can misread the user's words, for example a side or a volume. And a spoken or typed confirmation such as "confirm" or "cancel" must be recognised reliably, including in speech-to-text output. Users speak English and Hinglish (Hindi written in Latin script mixed with English). A model's reading cannot be tested without calling the model, and it cannot be used to cross-check the model's own proposal.

## Decision

The `gate` crate contains a deterministic intent parser, in `crates/gate/src/intent.rs`. It does three things:

1. It detects confirm and cancel phrases in speech and text.
2. It extracts the side, symbol, volume and prices the user actually said.
3. It cross-checks every `propose_*` call against those extracted values. A proposal that contradicts the user's words is rejected, and Jarvis asks the user again instead of proceeding.

The parser is tested against a corpus of more than 100 phrases, in English and Hinglish. The corpus is planned for `crates/gate/tests/intent_phrases.rs`.

Status: planned for Sprint 2.

## Consequences

- Confirm and cancel work without depending on the model, and the parser can be tested offline.
- A proposal with the wrong side or volume is caught before a preview is shown, which supports SPEC rule 9 (ask, never guess).
- The parser must be maintained as the phrase set grows. Phrases it does not know fall back to a clarifying question, not a guess.
- Mixed-language phrases need care. The corpus will record the phrases we support and the ones we do not.

## Alternatives considered

- **Rely on the model alone.** The model can misread a request, and its reading cannot be tested without a live model. It also cannot check itself.
- **A statistical NLU model.** It needs training data and more dependencies, and it is harder to explain or audit than rules.
- **English keyword matching only.** It would not handle Hinglish, which many users speak, and would fail on common mixed phrasing.
