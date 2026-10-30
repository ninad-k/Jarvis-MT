# ADR-0005: The LLM gets read tools and propose tools only

- Status: Accepted
- Date: 2026-10-30

## Context

SPEC rule 1 says the LLM can only call read tools and `propose_*` tools, and that no tool executes a trade. Sprint 1 ships the read tools (`get_price`, `get_account`, `get_positions`, `get_orders`, `get_history`) in a closed enum with no way to register a write tool. The trading features in Sprint 2 need the model to turn a request such as "buy 0.1 gold with SL 2300" into something the user can confirm. The model must not be able to send that order itself.

## Decision

- Sprint 2 adds four propose tools to the model: `propose_order`, `propose_modify`, `propose_close` and `propose_cancel`. Each one builds an `ActionPreview` through the action gate (ADR-0006 and the gate limits). Nothing the model can call writes to the bridge.
- A propose tool returns the preview text to the model. It never returns the confirmation token. The token is sent only to the browser, and only `POST /api/confirm/{token}` sends a write.
- The tool registry keeps the closed-enum design from Sprint 1. A regression test asserts that the tool specs sent to the model contain no write tool.

Status: planned for Sprint 2. The read-tool part is implemented.

## Consequences

- A bug in the model, a prompt injection, or a misread request cannot place an order on its own. A person must confirm a preview within its time limit.
- The model cannot see the token, so it cannot confirm for the user, even by mistake.
- Every trade passes through a single, testable path: the gate, then the token, then the write.
- The model's answer to "buy gold" depends on the preview, so it must explain the preview accurately. The preview text is the source for what the user is asked to confirm.

## Alternatives considered

- **Give the model a write tool and ask the user to confirm in chat.** A confirmation in chat is text the model could also produce. This breaks SPEC rule 1 and would rely on the model not making a mistake.
- **Let the model return a structured order that the code executes after a click.** This is close to what we chose, but the model would still hold a write capability. We preferred a model that cannot write at all.
- **No trade actions through the model.** The user would fill a form for every order. This removes the voice and chat use cases that the project exists for.
