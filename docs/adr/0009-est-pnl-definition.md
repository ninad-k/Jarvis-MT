# ADR-0009: Definition of estimated P/L in previews

- Status: Accepted
- Date: 2026-10-30

## Context

Every preview shows an estimated profit or loss (`est_pnl`) so the user can judge the action before confirming. The meaning differs by action. Closing a position realises its current floating profit or loss. A new order has no profit or loss yet, so its estimate depends on where the user sets the stop loss and take profit. The plan considered showing the text "n/a" when a level is missing, and the API has to return a typed value instead.

## Decision

- For **close** actions (`propose_close`, batch closes), `est_pnl.current` is the floating profit or loss of the positions being closed. This is the amount that would be realised.
- For **new orders** (`propose_order`), `est_pnl.at_sl` is the profit or loss if the stop loss is hit, and `est_pnl.at_tp` is the profit or loss if the take profit is reached. Both are computed from the symbol's tick value and the volume. A value is `null` when the user did not set that level.
- The plan does not yet define `est_pnl` for modify and cancel actions. Until a decision is recorded, those previews carry `null`.
- All figures are computed by the backend (ADR-0010), in the account currency.

Status: planned for Sprint 2.

## Consequences

- The field has one meaning for each action type, and the preview text can state it.
- Estimates for new orders assume the symbol's tick value stays the same, so they are approximate. They do not model commissions or swaps.
- `null` rather than a text value keeps the contract typed, so the HUD and the model handle a missing figure the same way.

## Alternatives considered

- **The string "n/a" when a level is missing.** It mixes text and numbers in one field, which makes the contract harder to type and to test.
- **Show no estimate for new orders.** Users would have less information before they confirm.
- **Estimate using the broker's margin and spread.** It is more precise for some brokers, but it needs more data and more code, and the simpler tick-value rule covers the common case.
