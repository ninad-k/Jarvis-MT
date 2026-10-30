# Support

Jarvis MT is maintained by one person, so support is best effort. The steps below get you the fastest useful answer.

## Where to go

| What you need | Where |
|---|---|
| A question or a how-to | [GitHub Discussions](https://github.com/ninad-k/Jarvis-MT/discussions). Search first; the answer may already be there. |
| A bug | [Open an issue](https://github.com/ninad-k/Jarvis-MT/issues/new/choose) and use the bug report template. |
| A feature idea | Start a thread in [GitHub Discussions](https://github.com/ninad-k/Jarvis-MT/discussions). Features that change the safety rules also need an ADR (see [CONTRIBUTING.md](CONTRIBUTING.md)). |
| A security problem | Do not open a public issue. Use private vulnerability reporting, as described in [SECURITY.md](SECURITY.md). |

## Before you ask

1. Read the README [Windows setup](README.md#windows-setup) and the troubleshooting table in the [runbook](docs/runbook.md#troubleshooting).
2. Check `http://127.0.0.1:8787/api/status`. It shows whether the bridge, the LLM provider and the mode are healthy.
3. Check the **Experts** tab in MT5 (Toolbox, Ctrl+T) and the console where the API runs.

## What to include in a bug report

- The Jarvis MT version (the API's `version` in `/api/status`, and the EA version in the Experts log).
- Windows version, MT5 build, and the LLM provider you use (for example Ollama with `qwen3.5:9b`).
- The steps you took, what you expected, and what happened.
- The relevant log lines from the API console and the Experts tab. Remove any login, account number, server name, password or API key before you paste them.

If the bug report template does not appear, include the points above in a plain issue.

## What we do not support

- **Trading advice.** We do not tell you what to trade, when, or how much.
- **Your broker, account or MT5 installation.** Contact your broker or MetaQuotes for account, server, symbol, login and terminal problems.
- **LLM provider accounts.** Billing, quotas, keys and model availability belong to the provider. We help with how Jarvis MT is configured to use a provider.
- **Modified builds.** We support the code in this repository at the version you are running. Forks and local changes are not supported here.
- **Live accounts.** Jarvis MT is developed and tested on demo accounts. Using it on a real account is at your own risk.

## Important

Jarvis MT is not financial advice and does not recommend trades. Use a demo account. Leveraged trading can lose money quickly.

The software is provided "as is", without warranty of any kind, as described in Sections 7 and 8 of the [Apache License 2.0](LICENSE). You are responsible for your trading decisions and for the settings you choose.
