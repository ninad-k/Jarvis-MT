# ADR-0017: Personas change how Arc MT sounds, not what it may do

- Status: Accepted
- Date: 2026-10-30

## Context

Users want to choose how Arc MT talks to them: a calm, formal voice (the persona Jarvis) or a warm, casual one (the persona Friday). Each needs its own Piper voice model, wake phrase for hands-free mode and HUD colours. Users may also want to add their own.

The safety model depends on the tools, the limits, the confirmation tokens, the kill switch and the audit log being the same for every conversation (SPEC.md, ADR-0005, ADR-0012). If a persona could change any of them, the review of the safety path would have to cover every persona, and a text file could weaken a rule. Switching persona must also keep the conversation and must leave a trail that shows which persona was active.

Limits are set in `config/jarvis.toml`. Users also want to tighten them from the web app, for example to trade less on a bad day, without being able to loosen them there.

## Decision

- **A persona is a config pack.** Each persona is a TOML file in `personas/`. It sets only its name, its personality text, its Piper voice (`voice_model`), its wake phrase and its HUD colours. Adding a persona needs no code change.
- **Everything else is shared.** Tools, limits, previews, confirmation tokens, the kill switch, the audit log and every safety rule are the same for all personas and live in code. Chat history and sessions are shared too, so switching persona keeps the conversation.
- **The personality sits inside a fixed frame.** `compose_system_prompt` builds the system prompt as: an identity line, the persona's personality in a delimited section, the shared rules verbatim, and a closing line that says the shared rules take precedence over the personality section. A personality text cannot replace the frame.
- **The audit records the persona.** Every audit row carries the persona that was active (the `audit.persona` column), so a trade or a read can be traced to the voice that was in use.
- **Parity tests guard the shared part.** Every shipped pack must load and validate. For every persona, the composed system prompt must contain the shared rules unchanged, and the tool set and the limits must be the same. A new persona must pass these tests before it is merged.
- **Limits can be tightened from the UI, with an authenticator code.** The Settings page can make each configured limit stricter: a lower `max_lot`, `max_positions` or `max_daily_loss`, fewer allowed symbols, fewer trading days or a shorter trading window. The UI can never make a limit looser; a looser value is refused with `limit_not_stricter`. The overrides are stored in SQLite, apply to every persona, and are checked with the same gate. Saving or resetting them needs a current TOTP code when login is on (ADR-0011). The effective limit is the configured value with the override applied, so it is never looser than the configuration.

## Consequences

- Adding a persona is a file change, reviewed as configuration. The shared rules are reviewed once.
- The audit trail says which persona was active, which helps when a reply sounds wrong.
- A persona cannot add a tool, a limit or a safety exception. Behaviour that differs by persona would need a code change and its own ADR.
- Every persona needs a Piper voice file of its own. Each one is a separate download of about 60 MB, pinned and checked by `scripts/fetch-models.ps1`. A persona whose voice file is missing runs with voice off.
- Personality text can make a reply sound more confident or more casual. It cannot change the numbers, which still come from the read tools and the backend totals (ADR-0010), or the confirmation step.
- The limits now have two sources, the configuration and the overrides. The gate uses the effective values, and the tests must cover both, so an override never widens a limit.
- Follow-up work: the `user_settings`, `limit_overrides` and `audit.persona` storage, the settings endpoints in `docs/api.md`, and the persona parity tests.

## Alternatives considered

- **Persona-specific limits (rejected).** A persona could allow a larger lot size or more symbols. Then whether a trade is allowed would depend on which voice was chosen, and each persona would need its own safety review. Limits are shared, so one review covers every persona.
- **Free-form persona prompts that replace the system prompt (rejected).** A pack could drop the shared rules, such as "propose, never send" (ADR-0005), or the instructions about figures. Composing the personality inside a fixed frame keeps those rules in every request, whatever a pack says.
- **Limits editable in the UI without restriction (rejected).** It would let the web app widen what the configuration owner set. Tighten-only changes give the user control over risk without a way around the configuration.
