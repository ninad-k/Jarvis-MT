# ADR-0007: Voice confirmation window starts after read-back

- Status: Accepted
- Date: 2026-10-30

## Context

SPEC rule 2 gives every preview a one-time token that is valid for 15 seconds. In a voice turn, Jarvis reads the preview aloud, and reading it can take 5 to 8 seconds. With a fixed 15-second window from issue, the user may have only a few seconds left to say "confirm". The window must still be short enough to limit the risk from a stale preview.

## Decision

- A preview made in a voice turn gets timing `after_readback`. Its 15-second window does not start when the token is issued. It starts when the browser reports that text-to-speech has finished reading the preview, through `POST /api/readback/{token}`.
- The window never extends beyond 30 seconds after the token was issued (`voice_token_cap_secs` in config).
- A preview created from the HUD (text or button) uses timing `immediate`, with the 15-second window from issue.

Status: planned for Sprint 2 (API) and Sprint 3 (voice).

## Consequences

- A user has time to hear the preview and then confirm, without the window growing without limit.
- The timer depends on the browser calling the read-back endpoint. If the call never arrives, the token still expires no later than 30 seconds after it was issued. The failure is safe, but the user may need to ask again.
- Two timings exist, so the API response and the audit log must show which one applied.

## Alternatives considered

- **A fixed 15-second window from issue.** This is simple, but reading the preview aloud leaves too little time for a confirmation, so the feature would be unusable by voice.
- **A window that starts at read-back with no cap.** A preview could then stay open for as long as the user wanted, which weakens SPEC rule 2.
- **A fixed 30-second window.** It would give more time, but it also doubles the period in which a stale preview can be confirmed.
