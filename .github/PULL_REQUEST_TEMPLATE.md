## Summary

<!-- What does this change and why? Two or three sentences. -->

## Linked issue

<!-- Closes #123. If there is no issue, explain why one is not needed. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor or cleanup (no behaviour change)
- [ ] Documentation
- [ ] CI, build or tooling
- [ ] Change to the safety rules, gate, auth or bridge protocol (needs an ADR)

## How it was tested

<!-- Commands you ran and what you saw. For anything that touches MT5, say whether you used a demo account. -->

## Checklist

- [ ] Tests added or updated, and they fail without the change.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass (or `pwsh scripts/check.ps1`).
- [ ] For web changes: `pnpm -C web check`, `pnpm -C web test` and `pnpm -C web build` pass.
- [ ] Documentation updated (README, `docs/`, or code comments) where behaviour changed.
- [ ] `CHANGELOG.md` updated under `[Unreleased]` for any user-visible change.
- [ ] No secrets, API keys, tokens or account numbers in the diff, the tests, the fixtures or the screenshots.
- [ ] The safety rules in [SPEC.md](../SPEC.md) are untouched, or an ADR in `docs/adr/` explains and justifies the change.
- [ ] New dependencies are justified in the description and pass `cargo deny check` / `pnpm audit`.
