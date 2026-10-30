# ADR-0008: Batch actions need a second confirmation with a fresh token

- Status: Accepted
- Date: 2026-10-30

## Context

SPEC rule 4 says batch actions, such as "close all" or "close all nasdaq longs", need a second confirmation that repeats the count. A batch can affect many positions at once, and a mistaken batch is costly. The design had to decide whether the second confirmation shares the first token's time window or gets its own.

## Decision

- The first confirmation of a batch action does not execute it. It returns a fresh 15-second second-stage token that is bound to the same preview. The response message repeats the count, for example "Confirm again to close 3 positions".
- The second confirmation must use that second-stage token. Only then do the batch commands run, one after another.
- Cancelling either token cancels both.

Status: planned for Sprint 2.

## Consequences

- The user gets a full 15 seconds for the second confirmation, and the count is shown twice, which reduces the chance of confirming the wrong batch.
- A batch takes longer to run than a single action, which is acceptable for an action the user has confirmed twice.
- The API and the audit log must track both stages, using the `confirm` and `confirm_second` kinds.

## Alternatives considered

- **One confirmation for everything.** This does not satisfy SPEC rule 4.
- **One token whose window must cover both confirmations.** The two confirmations would have to fit in the original 15 seconds, which is too tight with voice read-back.
- **A second confirmation that only repeats a code, with no fresh token.** A reused token is easier to replay, and it does not give the user a new, time-limited decision.
