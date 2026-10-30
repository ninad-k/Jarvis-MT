# ADR-0001: Record architecture decisions

- Status: Accepted
- Date: 2026-10-30

## Context

Jarvis MT makes decisions that are easy to forget and costly to revisit. Some of them protect real money: which component may write to MT5, how the bridge is secured, and what the LLM may call. Others are trade-offs, such as vendoring binaries instead of downloading them. Without a record, contributors reopen settled questions or break a rule without knowing it exists.

## Decision

We record significant architecture and safety decisions as short numbered Markdown records in `docs/adr/`, using a MADR-style layout (status, context, decision, consequences, alternatives). The process and template are in [README.md](README.md). A record that is accepted is not rewritten. A changed decision gets a new record that supersedes the old one.

## Consequences

- New contributors can see why the system looks the way it does, which helps them respect the core rules in SPEC.md.
- Writing a record takes time, so we only write them for decisions with lasting effect.
- Records can become outdated. The superseded status and the index keep the history clear.

## Alternatives considered

- **No records, decisions in commit messages.** Commit messages are hard to find by topic and do not record alternatives.
- **A single long DECISIONS file.** It grows without limit and makes review of individual decisions harder.
- **A wiki.** A wiki is not versioned with the code, so it can drift away from the repository state.
