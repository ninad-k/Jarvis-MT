# ADR-0013: Timed-out writes have an unknown outcome and are never retried

- Status: Accepted
- Date: 2026-10-30

## Context

The bridge has a timeout of 3 seconds by default (ADR-0002). A write request can reach the EA, and the EA can execute the trade, while the reply is lost or arrives after the timeout. From the backend's side, the result is then unknown: the trade may or may not exist. A retry with a new request could open a second position. Retrying the same request is only safe if the EA deduplicates it, and only while its reply cache still holds the entry (ADR-0012).

## Decision

- Once a write request has been sent, a timeout or transport failure is final for that request. The backend never retries it.
- The API returns `504 outcome_unknown`. The response tells the user to check positions, and the backend refreshes the position list so the HUD shows the current state.
- The client's existing rule still applies: it resends a request only when the send itself failed before the request was delivered (for example, a reused socket after an EA restart). A failure after the send is never retried.
- The audit log records the write as sent, with no reply, so the outcome can be investigated.

Status: planned for Sprint 2. The client rule for resending after a failed send is implemented in Sprint 1.

## Consequences

- No duplicate trades come from automatic retries.
- The user may need to check MT5 after an `outcome_unknown` response. The interface must make that clear.
- A timeout on a write can happen even when the trade succeeded, so the HUD must not assume failure.

## Alternatives considered

- **Retry with the same request id.** The EA would return the cached reply if it still had one. That only works while the cache is intact, which is not guaranteed across an EA restart, so it cannot be the only protection.
- **Retry with a new request id.** This can open a second position, which is the failure we must avoid.
- **Report the write as failed.** The user would believe no trade happened when one may have. This is a worse error than an unknown outcome.
- **A longer timeout for writes.** It reduces the chance of an unknown outcome but does not remove it, and it would keep the user waiting while the market moves.
