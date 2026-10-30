# ADR-0010: Rust computes every money total

- Status: Accepted
- Date: 2026-10-30

## Context

Users ask the chat for totals such as "what did I make this week?" or "what's my open profit?". A language model is not reliable at arithmetic over long lists of deals. In Sprint 1 testing, the model added up weekly profit and loss from the deal list and got the total wrong. The error was in the model's arithmetic, not in the data the bridge returned. Money totals are a core part of what the user relies on, and SPEC rules 5 and 7 (limits and daily loss) depend on the same numbers.

## Decision

The backend computes every money figure, and the model only quotes it.

- `get_history` returns a `summary` computed over every deal in the requested range, not only the deals shown. It includes deal count, net profit (profit plus commission plus swap), gross profit, commission and swap, overall and per symbol. Balance and credit deals are excluded from trade totals. `summary.complete` is `false` when the range held more than 500 deals.
- `get_positions` returns a `total_profit` computed by the backend.
- Previews (`est_pnl`, ADR-0009) and the daily-loss check (`max_daily_loss`) are computed in Rust.
- The tool descriptions tell the model to quote these numbers and never to add up deals itself.
- Figures are rounded to two decimal places in the backend.

Sprint 1 implements the first two points. The preview and daily-loss parts are planned for Sprint 2.

## Consequences

- Totals are correct by construction and covered by tests, such as the per-symbol and balance-exclusion tests in `crates/api/src/tools.rs`.
- The model's job becomes reading and explaining numbers, which is much easier to get right.
- A total over a range with more than 500 deals is marked incomplete, so the model has to say so.
- The backend's rules now define the money logic, which makes changes to it a higher-stakes review.

## Alternatives considered

- **Let the model sum the deals.** This was the cause of the Sprint 1 error, so it is ruled out.
- **Compute totals in the browser.** The totals would not be in the audit log, and the model would not see the same numbers the user sees.
- **Compute totals in the EA.** The EA is kept to request handling and validation, and a computed total would still need the same tests on the Rust side.
