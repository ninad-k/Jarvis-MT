# Contributing to Arc MT

Thanks for your interest in Arc MT, a voice and chat tool for trading on MetaTrader 5. Bug reports, documentation fixes, tests and features are all welcome. This guide explains how to set up the project, what we expect in a change, and how changes are reviewed.

Before you start, read [SPEC.md](SPEC.md) (the product rules) and [docs/architecture.md](docs/architecture.md). Questions go to [GitHub Discussions](https://github.com/ninad-k/Jarvis-MT/discussions); see [SUPPORT.md](SUPPORT.md).

Please follow the [Code of Conduct](CODE_OF_CONDUCT.md) in all project spaces.

## Ground rules

Arc MT can place real orders in MetaTrader 5, so safety comes before features.

- **The core rules in [SPEC.md](SPEC.md) are non-negotiable.** The LLM only reads data and proposes previews. A trade reaches MT5 only after a user confirms a short-lived preview. The kill switch blocks every write. Demo is the default. Account credentials never reach the LLM or the browser. Every step is written to the audit log.
- **Changes to the safety path need tests and an ADR.** This covers the action gate, confirmation tokens, the kill switch, authentication, and the code that writes to MT5 (the bridge write commands and the `JarvisBridge` EA). Such a change needs automated tests that fail if the rule is broken, and an accepted ADR in [docs/adr/](docs/adr/README.md).
- **Do not give the LLM a tool that writes.** Do not let the EA act without a request.
- **Never commit secrets.** `.env`, API keys, the JWT secret and `config/jarvis.toml` are gitignored. Check your diff anyway before you push.
- **Live tests use a demo account only.** Never run a test that opens, changes or closes positions on a real account. The live write tests (`crates/bridge-client/tests/live_trading.rs`) abort unless the account is a demo, use the minimum lot and must leave nothing open.

## Development setup

Follow the **Windows setup** section of the [README](README.md). It covers Rust, Node.js, pnpm, MT5 with a demo account, PowerShell 7, the LLM options, and the EA install.

You can work without MT5. The mock bridge serves canned data on the bridge port, so the API and web app run end to end:

```powershell
cargo run -p jarvis-bridge-client --features mock --example mock_server
```

Stop the mock before you attach the real EA, because both use port 5555. See the README section "Develop without MT5".

## Repository layout

| Path | Contents |
|---|---|
| `crates/api` | `jarvis-api`: HTTP API, read tools, chat loop, config, audit log |
| `crates/bridge-client` | Protocol types, ZeroMQ client, mock bridge, examples |
| `crates/llm` | LLM provider trait and Anthropic, OpenAI and Ollama clients |
| `crates/gate`, `crates/voice` | Action gate (limits, previews, tokens, kill switch, intent parser) and voice (Whisper STT, Piper TTS) |
| `mql5/` | `JarvisBridge` EA, JSON and protocol headers, vendored mql-zmq ([THIRD_PARTY.md](mql5/THIRD_PARTY.md)) |
| `web/` | SvelteKit web app (the HUD) |
| `config/` | `jarvis.example.toml`, the non-secret settings template |
| `scripts/` | `install-ea.ps1` and `compile-ea.ps1` |
| `docs/` | Architecture, API, protocol and runbook docs, plus [docs/adr/](docs/adr/README.md) |
| `tasks/` | Sprint plans and task lists |

## Branching and pull requests

- Work on a feature branch and open a pull request against `main`. Do not push to `main` directly.
- Name branches with a short prefix and a topic, for example `feat/gate-limits` or `fix/bridge-timeout`.
- Keep each pull request to one logical change. Large work is easier to review in steps.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>
```

Types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`, `build`, `perf`, `security`.

Scopes: `api`, `gate`, `bridge`, `llm`, `voice`, `web`, `desktop`, `ea`. The scope is optional, for example `docs: explain the kill switch`.

Mark breaking changes with `!` after the type or scope, or with a `BREAKING CHANGE:` footer.

A `Signed-off-by:` line (Developer Certificate of Origin) is optional. It is not required.

## Code style

- **Rust:** format with `cargo fmt --all`. Lint with `cargo clippy --workspace --all-targets -- -D warnings`. Warnings fail the build. Library crates return typed errors (`thiserror`). Only the `api` binary uses `anyhow`. No `unwrap()` or `expect()` outside tests and startup config.
- **Money and volume:** values are rounded to the symbol's volume step and digits before any write to MT5.
- **MQL5:** the EA must compile with zero errors and zero warnings in our code (`pwsh scripts/compile-ea.ps1`).
- **Web:** TypeScript and Svelte, checked with `pnpm -C web check`. The repo has no Prettier configuration yet. If one is added, format with it.
- **Protocol and HTTP contracts:** change [docs/protocol.md](docs/protocol.md) and [docs/api.md](docs/api.md) in the same pull request as the code, and update the golden fixtures.

## Testing

| Check | Command | Needs |
|---|---|---|
| Unit and contract tests | `cargo test --workspace` | Nothing |
| Mock bridge tests | Included in the command above | Nothing |
| Lints | `cargo clippy --workspace --all-targets -- -D warnings` | Nothing |
| Formatting | `cargo fmt --all --check` | Nothing |
| EA compile | `pwsh scripts/compile-ea.ps1` | MT5 installed |
| Web checks | `pnpm -C web check` and `pnpm -C web build` | `pnpm -C web install` |
| Live demo tests (opt-in) | `cargo test -p jarvis-bridge-client --features mock -- --ignored --test-threads=1` | MT5 demo account with the EA attached |

Expectations:

- Every core rule in SPEC.md has at least one automated test that fails if the rule is broken.
- Bug fixes include a regression test.
- Tests do not need a real broker, a real account or network access, except the opt-in live tests.
- Live tests are marked `#[ignore]` and run only on demand against a demo account. Set `JARVIS_LIVE_SYMBOL` to a symbol your broker uses.
- Browser smoke tests: `pnpm -C web test:e2e` (Playwright, Chromium, against the mock backend).

To run every check at once, use `pwsh scripts/check.ps1` (flags: `-SkipWeb`, `-SkipSecurity`, `-SkipWhisper`, `-SkipDesktop`).

## Documentation and ADRs

- Update the docs in the same pull request as the behaviour they describe: the README, [docs/](docs/), and `config/jarvis.example.toml` when a setting changes.
- Write an ADR for any decision that changes a core rule, a wire contract, a security boundary, or adds a dependency or binary. See [docs/adr/README.md](docs/adr/README.md) for the template and the process.
- Describe only features that exist or are explicitly planned. Mark planned work as "planned".

## Pull request checklist

- [ ] The title follows Conventional Commits.
- [ ] Tests cover the change, and `cargo test --workspace` passes.
- [ ] `cargo fmt --all --check` and `cargo clippy ... -D warnings` pass.
- [ ] If `web/` changed: `pnpm -C web check` passes.
- [ ] If `mql5/` changed: the EA compiles with zero errors and zero warnings.
- [ ] The change adds no tool or path that lets the LLM or the EA write to MT5 without a confirmed preview.
- [ ] If the change touches the gate, tokens, the kill switch, authentication or the EA write path: tests are included and the ADR is linked.
- [ ] Protocol or HTTP changes update the docs and fixtures.
- [ ] `CHANGELOG.md` has an entry under `[Unreleased]` for user-visible changes.
- [ ] Nothing in the diff contains secrets, account numbers, logins, server names or personal data, including test fixtures and logs.

## Release process

1. Move the `[Unreleased]` entries in `CHANGELOG.md` under a new version heading with the release date.
2. Update the version numbers that the API, the EA and the web app report, so they match.
3. Update the compare links at the bottom of `CHANGELOG.md`.
4. Create and push a tag named `vX.Y.Z` on `main`. A tag that matches `v*` triggers the release workflow (`.github/workflows/release.yml`).

Versioning rules are in [GOVERNANCE.md](GOVERNANCE.md).

## Licensing of contributions

The project is licensed under the [Apache License 2.0](LICENSE). Unless you state otherwise, any contribution you submit is licensed under Apache-2.0, the same license as the project (inbound = outbound). You keep the copyright to your contribution. This follows Section 5 of the Apache License 2.0.

Third-party code must be compatible with Apache-2.0. Vendored code needs its license file and an entry in [mql5/THIRD_PARTY.md](mql5/THIRD_PARTY.md). New binaries need maintainer approval before they are committed, as SPEC.md requires.

## Reporting security issues

Do not open a public issue for a security problem. Use private vulnerability reporting, as described in [SECURITY.md](SECURITY.md).

## Questions

Use [GitHub Discussions](https://github.com/ninad-k/Jarvis-MT/discussions) for questions. The maintainer is [@ninad-k](https://github.com/ninad-k) on GitHub. Decision making is described in [GOVERNANCE.md](GOVERNANCE.md).
