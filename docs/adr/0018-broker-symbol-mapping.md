# ADR-0018: Broker symbol mapping with canonical names and per-broker layered maps

- Status: Accepted
- Date: 2026-10-30

## Context

Every broker names instruments differently. The same gold contract is `XAUUSD_`, `GOLD_CASH`, `XAUUSD.pr` or `XAUUSDm` depending on the broker, and indices are the worst case: `US100_Spot`, `USTEC`, `UT100Roll` and `NDXcash` all mean the Nasdaq 100. Before this decision the API sent names through one configured `suffix`, which worked for one broker. A single suffix does not describe a broker's naming: one broker adds `_` to every name, another adds `.pr` to its FX pairs and metals but uses bare names for some other instruments, and a third uses `cash` tails for its indices while its FX pairs keep their own names. A user who switched account in MT5 got no change in names, so an order could go to an unknown symbol, or, worse, to a different instrument that happened to share a name.

Arc MT's limits, watchlist, chat and gate are the safety path (SPEC.md, ADR-0005, ADR-0012). They must mean the same instrument on every broker. The MT5 terminal can report the broker's registered company name (`ACCOUNT_COMPANY`) and its full symbol catalog through the bridge (protocol version 2, EA 0.4.0 with `get_broker` and `list_symbols`, see [protocol.md](../protocol.md)). Market Watch alone is not enough, because a broker's instruments are not all shown there.

Some safety-relevant facts shape the rules. A wrong symbol means a wrong instrument, so a mapping must be explicit and reviewable. A preview is made for a particular account, so the account must not change between preview and confirmation (ADR-0012). A mapping change is a change to what the user trades, so it needs the same second factor as a change of limits (ADR-0011, ADR-0017).

## Decision

- **Canonical names everywhere.** The canonical names in [config/instruments.toml](../../config/instruments.toml) (`XAUUSD`, `US100`, `US30`, `EURUSD`, ...) are the names used by the limits, the watchlist, chat, the gate, the web UI and the API responses. Only the calls to the EA use a broker's own name. Spoken names (`gold`, `nasdaq`) and generic aliases (`GOLD`, `NAS100`, `USTEC`) resolve to a canonical name first.
- **Per-broker maps in three layers, keyed by the MT5 company name.** The user layer (`data/broker_symbols.user.toml`, written from Settings) ranks above the curated layer (`config/broker_symbols.toml`, checked into the repository) and the generated layer (`data/broker_symbols.generated.toml`, written by detection). Each layer holds explicit entries and regular-expression patterns. Explicit entries of every layer are tried before any pattern, so a pattern never hides an explicit entry. Names that match nothing pass through unchanged, and the configured suffix is a last resort for a broker with no map at all.
- **Detection from the full catalog.** When Arc MT sees a broker it has not mapped in this run, it reads the broker's whole symbol list through the EA and runs a deterministic matcher over it. The matcher finds the broker's affix from the seven FX majors, scores the candidates (tradability, Market Watch visibility, name fit, description keywords, cash and spot contracts) and rejects dated futures, stocks, ETFs and disabled symbols. The result, with its candidates and scores, is shown in Settings for review. Detection runs at start-up, on a switch to a broker not yet seen in this run, and on demand.
- **Account switches are noticed and bound to previews.** Each successful status check, the startup check, a background check every 30 seconds, and the points where a proposal or a confirmation is handled, all ask the bridge for the broker and compare its company with the last one seen. A preview records the company and the trade server it was made on. A confirmation on a different account is refused with `409 broker_changed`, the token is cancelled and the refusal is audited, and nothing is sent.
- **Overrides need a fresh authenticator code when login is on.** Saving or resetting an override through `PUT /api/symbols/map` requires a TOTP code when login is enabled, because a wrong mapping can send an order to the wrong instrument. Re-detection needs no code: it only reads the broker and writes the generated layer. With login off, no code is asked for, as with the other settings.
- **Unmapped instruments are refused, not guessed.** An instrument whose resolved name the broker does not have is flagged in Settings and under "Unmapped on this broker". A request for it reaches the EA with that name, and the EA answers `unknown symbol`, so nothing is sent. Arc MT never picks a similar symbol on its own.

## Consequences

- Switching broker or account in MT5 needs no configuration change and no restart. Limits, the watchlist and chat keep their meaning on the new broker.
- Adding a broker is a reviewed change to a TOML file, or a review of a detected map in Settings. Neither changes code.
- The curated file is maintained by hand and must be kept honest. Its entries are checked when loaded, but their correctness is not. Entries marked unverified stay in force until someone checks them, and they outrank detection for the names they cover.
- Detection is a heuristic. Its errors are visible (the candidates and their reasons are shown, and each row shows its source), and an override fixes them, but it can pick a symbol that exists and is the wrong instrument (for example a mini or a different listing). Reviewing the Settings rows after the first connection to a broker is part of the operating procedure.
- One malformed layer file turns all maps off. The server still starts, names pass through with the configured suffix, detection is off and a warning is logged. This keeps the server running, but the operator must fix the file. The alternative, refusing to start, would stop a demo session for a typo in an unrelated broker's map.
- Catalog snapshots (`data/broker-catalogs/`) hold the broker's symbol names, descriptions and paths, with the trade server name as their label. They are broker data. The account's login and account name are never written to them.

## Alternatives considered

- **A single global suffix (rejected).** A single `[symbols] suffix` cannot describe brokers that use different affixes for different instrument classes, or that use bare names for some instruments. It remains only as a last resort.
- **Symbols guessed by a language model (rejected).** A model could propose broker names from the canonical names. The result is not deterministic, cannot be checked against the broker's catalog before use, and would put unverified names into the path that sends orders. Names are instead checked against the broker's own catalog, and the choice is shown for review.
- **Only the symbols in Market Watch (rejected as partial).** Market Watch shows what the user has selected, and many brokers' instruments are not in it until someone selects them. Visibility is used as a score, not as a filter, so an instrument that is only in the full catalog can still be found.
- **Only hand-written maps (rejected).** Every new broker would need an entry written and checked before its first use, and the maintainer could not check every broker. Detection gives a starting point for each broker, and the curated layer records the ones that were checked.
