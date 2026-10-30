# Configuration

Jarvis reads two places at startup:

- **`config/jarvis.toml`**: non-secret settings. It is gitignored, so start from the template:
  `Copy-Item config\jarvis.example.toml config\jarvis.toml`.
- **`.env`**: secrets and the log level. It is loaded from the working directory and is gitignored.
  Start from `Copy-Item .env.example .env`.

Changes take effect after a restart of `jarvis-api`. The web UI is read from `static_dir` at startup, and
the server hashes its inline scripts once, so rebuild the web UI and restart after each change.

## Which file is used

1. The `--config <file>` flag (also accepted after the subcommand, for example `jarvis-api user list -c <file>`).
2. Otherwise the `JARVIS_CONFIG` environment variable.
3. Otherwise `config/jarvis.toml`, relative to the working directory. Start from the repository root, or
   set `JARVIS_CONFIG` to an absolute path. The Windows service always gets an absolute `--config` path.

A missing file stops the server with a message that tells you to copy the example.

## Startup validation

The server refuses to start, and says why, when:

- `mode` is not `demo` or `live`, or the account reports a real trade mode while `mode = "demo"`.
- `bridge.endpoint` is not `tcp://` on a loopback host (`127.0.0.1`, `::1` or `localhost`).
- There is no usable bridge key: neither `JARVIS_BRIDGE_KEY` nor `bridge.key_file` is set, the file cannot be read, or the key is not 64 hexadecimal characters (and not all zeros). The message points at `scripts/install-ea.ps1`. The key is never printed.
- `server.bind` is not loopback and TLS (`tls_cert` and `tls_key`) and `[auth] enabled = true` are not both set.
- Only one of `tls_cert` and `tls_key` is set, or a PEM file cannot be read.
- `[auth] enabled = true` and `JARVIS_JWT_SECRET` is missing, shorter than 32 bytes, or guessable: fewer than 16 different byte values, a run of 8 equal bytes, or a block that repeats (`abcdabcd...`). Base64 or hex text is judged on the bytes it decodes to. The message prints the command that generates a good one (the same as in `.env.example`).
- `llm.providers` is empty, two providers share a name, or `llm.default` names no provider.
- `[trading]` or `[limits]` values are out of range (see below).

A bridge that is down, a missing voice model, or a missing LLM key only logs a warning. The server still starts.

## `mode`

| Key | Type | Default | Effect |
|---|---|---|---|
| `mode` | `"demo"` or `"live"` | `"demo"` | Operating mode. In `demo`, startup fails when the MT5 account is real, and chat returns `409 mode_mismatch` if the account changes to real while the server runs. `live` is accepted only when you set it on purpose, and the web UI then shows a LIVE banner. |

Safety: keep `demo` for every test. Do not change `mode` to get past a `mode_mismatch`. Log MT5 into the demo account instead.

## `[server]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `bind` | `host:port` | `"127.0.0.1:8787"` | Address the HTTP and WebSocket server listens on. Any non-loopback address needs TLS and login (see startup validation). Use `0.0.0.0:8787` only with TLS, login and a firewall rule. |
| `static_dir` | path | unset | Folder with the built web UI, usually `"web/build"` (`pnpm -C web build`). The server serves it at `/` and falls back to `index.html` for client-side routes. Unset means the API only, which suits the Vite dev server. |
| `tls_cert` | path | unset | PEM certificate chain. Set it with `tls_key` to serve HTTPS and WSS. |
| `tls_key` | path | unset | PEM private key. It must not be encrypted. Keep it out of Git (the `certs/`, `*.pem` and `*.key` patterns are gitignored). |
| `dev_origins` | list of origins | `[]` | Browser origins (`"http://localhost:5173"`) allowed to call the API with the session cookie (CORS, and the WebSocket `Origin` check). Empty by default: the web UI is served by the API, and `pnpm -C web dev` goes through the Vite proxy, which is same-origin. Anything listed can drive the whole API, so list only origins you control. Entries are `scheme://host[:port]` with no path or wildcard. |
| `header_read_timeout_secs` | integer 1 to 120 | `10` | How long a client has to send the headers of a request. It also closes a keep-alive connection that has been idle that long, and the TLS handshake has its own 10 s limit. A client that trickles its headers in cannot hold a connection open (finding NET-1). |
| `request_timeout_secs` | integer 1 to 600 | `30` | How long a request may take to be answered, an upload of the body included. Applies to every route except the next row and the WebSocket (an open `/api/ws` has no limit; its upgrade is answered at once). A request that runs out gets `408 request_timeout`. |
| `long_request_timeout_secs` | integer, `request_timeout_secs` to 3600 | `200` | The same for the routes that wait for a language model, the speech engines or a batch of trades: `/api/chat`, `/api/voice/*` and `/api/confirm/*`. A chat turn can make up to five model calls, so keep this above the `timeout_secs` of your slowest provider; the server logs a warning at startup when a provider is not shorter. A confirmed trade keeps running after its request is stopped. |
| `max_connections` | integer 1 to 100000 | `256` | Open connections accepted at once, counted before any TLS handshake. Another connection is closed at once, unanswered. Open WebSockets (at most 16) count as connections. |
| `chat_rate_limit_per_minute` | integer 1 to 10000 | `20` | `POST /api/chat` requests per minute for one login session (per client address when login is off). Over it: `429 rate_limited` with `Retry-After`, and the model is not called (finding NET-2). |
| `transcribe_rate_limit_per_minute` | integer 1 to 10000 | `20` | The same for `POST /api/voice/transcribe`. |

When TLS is on, responses carry `Strict-Transport-Security`, the session cookie is `Secure`, and the CSP upgrades insecure requests.
The CSP lets the page connect to its own origin only (`connect-src 'self'`), which covers the WebSocket in Chrome, Edge and Firefox; see [security](security/threat-model.md).

## `[bridge]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `endpoint` | `tcp://<loopback>:<port>` | `"tcp://127.0.0.1:5555"` | Where the API connects to the `JarvisBridge` EA. Must be loopback. Must match the EA input `InpEndpoint`. |
| `timeout_ms` | integer > 0 | `3000` | Time to wait for each EA reply. A write that times out is not retried and is reported as `outcome_unknown`. Raise it only if replies are slow for a reason you understand. |
| `key_file` | path | unset | The shared bridge key (finding BR-1, [ADR-0016](adr/0016-authenticated-bridge-protocol.md)): a text file holding 64 hexadecimal characters, 32 random bytes. `scripts/install-ea.ps1` creates it as `<MT5 data folder>\MQL5\Files\JarvisMT\bridge.key` and prints the path. The EA reads the same file when it starts. Without the key the EA refuses every request, so the API **refuses to start** without one. `JARVIS_BRIDGE_KEY` overrides this setting. Write the path as a TOML literal string (single quotes), for example `key_file = 'C:\Users\you\AppData\Roaming\MetaQuotes\Terminal\<id>\MQL5\Files\JarvisMT\bridge.key'`. Only your own Windows account should be able to read the file; the MT5 data folder under `%APPDATA%` already is. |

Every message to and from the EA is signed with this key, so any other program on the PC that does not hold it cannot place orders or read the account, and it cannot answer in the EA's place. See [protocol.md](protocol.md#authentication). To change the key, see [Rotating the bridge key](runbook.md#rotating-the-bridge-key).

## `[database]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `url` | SQLite URL | `"sqlite://data/jarvis.db"` | The audit log, users and revoked sessions. Created on first start, with migrations applied at every start. The path is relative to the working directory. The file holds password hashes and encrypted TOTP secrets, so limit who can read it. |

## `[symbols]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `suffix` | string | `""` | Appended to every resolved symbol, for example `".m"`, or `"_"` for brokers that use that style. Whitespace is trimmed. |
| `watchlist` | list of strings | `[]` | Symbols shown in the web watchlist, written without the suffix. Entries that are empty after trimming are dropped. |
| `aliases` | table `spoken = "BROKER"` | `{}` | Spoken or typed names mapped to broker symbols, before the suffix is added. Keys are matched case-insensitively and whitespace is collapsed, so `"Dow  Jones"` matches `"dow jones"`. |

Write `[limits] allowed_symbols` and the watchlist without the suffix. The loader resolves each entry the same
way as a spoken name (alias lookup, then the suffix), so `"XAUUSD"` in `allowed_symbols` becomes `XAUUSD.m`
when `suffix = ".m"`. The suffix is not added twice if an entry already ends with it.

Unknown symbols are reported as `unknown symbol`. The API never picks a different symbol for you.

## `[llm]` and `[[llm.providers]]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `llm.default` | provider name | required | Provider used when a chat request does not name one. Must match a provider `name`. |
| `name` | string | required | Name used in the UI and in requests. Unique. |
| `kind` | `"ollama"`, `"anthropic"` or `"openai"` | required | Which API the provider speaks. |
| `model` | string | required | Model identifier, for example `qwen3.5:9b` for Ollama. |
| `base_url` | URL | `http://127.0.0.1:11434` (Ollama), `https://api.anthropic.com` (Anthropic), `https://api.openai.com/v1` (OpenAI) | Override the endpoint. |
| `api_key_env` | env var name | unset | Name of the environment variable that holds the key, for example `"ANTHROPIC_API_KEY"`. Required for `anthropic` and `openai`; ignored for `ollama`. The key itself never goes in the TOML file. A missing variable makes that provider unavailable. |
| `timeout_secs` | integer | `60` | Per-request timeout for the provider. The example sets `180` for Ollama, because the first reply after a cold start loads the model. |
| `max_tokens` | integer | `1024` | Upper bound on the reply length. |

Privacy: messages you type and the MT5 data the tools return (prices, positions, deals) are sent to the
provider. A cloud provider sends them off the PC. Ollama keeps them local. The model never receives login,
name or server details.

## `[limits]`

These apply before any preview is shown. Without a `[limits]` section nothing can be traded: the gate fails closed.
When the section is present, `max_lot`, `max_positions`, `max_daily_loss` and `allowed_symbols` are required.
`trading_hours` is optional.

| Key | Type | Effect and safety |
|---|---|---|
| `max_lot` | number > 0 | Largest volume of one **new** order. Closes are never limited by it. |
| `max_positions` | integer >= 1 | Open positions plus pending orders, counted after the new order. |
| `max_daily_loss` | number > 0 | In account currency. New orders are blocked once today's realised plus floating P/L is at or below minus this amount. The day starts at server midnight. |
| `allowed_symbols` | list of strings | Symbols that new orders may use. An empty list blocks all new orders. |
| `trading_hours.days` | list of `"mon"` to `"sun"` | Days when new orders are allowed. The list must not be empty. |
| `trading_hours.start`, `end` | `"HH:MM"` | Window in broker server time, start inclusive and end exclusive. They must differ. If `start` is later than `end`, the window crosses midnight. Remove the whole `[limits.trading_hours]` table to allow any time. |

`max_daily_loss`, `trading_hours`, `allowed_symbols` and `max_positions` apply to new orders only. Closing, modifying and
cancelling stay allowed when those limits are reached, because they reduce or keep risk. The kill switch blocks them too.

## `[trading]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `token_ttl_secs` | 1 to 120 | `15` | How long a confirmation token is valid. Limits and the market are checked again when you confirm, so a preview that has become impossible in those seconds (another preview was confirmed, the loss limit was reached) is refused with `422 gate_rejected`; see [api.md](api.md#previews-and-confirmation-sprint-2). |
| `voice_token_cap_secs` | integer >= `token_ttl_secs` | `30` | For a voice turn, the window starts when the spoken read-back finishes, and never runs past this many seconds after the preview was made. |
| `deviation_points` | 0 to 1000 | `20` | Largest slippage allowed for market orders and closes, in points. |
| `kill_switch_engaged` | boolean | `false` | Start with every write blocked. Independently of this setting, a kill switch that was engaged when the server stopped is saved in the database and stays engaged after a restart until it is released with a login session and a fresh authenticator code. |

## `[auth]`

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `enabled` | boolean | `true` | Requires a login session for every `/api` route except health and login. Turn it off only on a loopback-only machine: the server then logs a warning that anyone who can reach it can trade. |
| `session_hours` | 1 to 168 | `12` | Session lifetime. |
| `cookie_secure` | boolean | unset | Forces the `Secure` cookie attribute. Unset means `Secure` whenever the server serves TLS itself. Set it to `true` when a reverse proxy terminates TLS in front of a plain-HTTP loopback port. |

The signing secret comes from `JARVIS_JWT_SECRET`, not from this file. See the environment variables below.

## `[voice]`

The whole section is optional. Without it, voice is off. If the section is present, `enabled` defaults to `true`.

| Key | Type | Default | Effect and safety |
|---|---|---|---|
| `enabled` | boolean | `true` | Turns voice on or off. |
| `whisper_model` | path | required when enabled | Whisper model file, for example `models/whisper/ggml-base.en.bin` (fetched by `scripts/fetch-models.ps1`). Used only by a build with the `whisper` feature. |
| `language` | string | `"en"` | Language passed to Whisper. |
| `threads` | integer | `4` | CPU threads for Whisper. |
| `piper_exe` | path | required when enabled | Piper executable, for example `models/piper/piper.exe`. |
| `piper_voice` | path | required when enabled | Piper voice model, for example `models/piper/en_US-lessac-medium.onnx`. |
| `max_seconds` | number greater than 0, at most 120 | `30` | Longest recording accepted for transcription. |

A broken engine does not stop the server. Speech-to-text and text-to-speech are switched off separately, and
the reason is written to the log. `GET /api/status` reports `voice.stt` and `voice.tts`.

## `.env` and environment variables

| Variable | Required | Effect and safety |
|---|---|---|
| `JARVIS_JWT_SECRET` | Yes, when `[auth] enabled = true` (the default), and for every `jarvis-api user` command | Signs session tokens and derives the key that encrypts TOTP secrets. At least 32 bytes, with at least 16 different byte values and no repeated block (a value that is Base64 or hex text is judged on the bytes it decodes to); the server refuses anything weaker. Generate a random value (see below). Changing it logs everyone out and makes stored TOTP secrets unreadable, so run `jarvis-api user reset-totp <name>` for each user afterwards. Never commit it. |
| `JARVIS_BRIDGE_KEY` | Yes, unless `[bridge] key_file` is set | The shared bridge key as 64 hexadecimal characters. When set and not empty it wins over `key_file`; if it is set but not a valid key, the server stops instead of falling back to the file. Same secret as the content of `bridge.key`. Never commit it, and never paste it into an issue or a chat. |
| The variable named by `api_key_env`, for example `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` | Only for that provider | The provider API key. Keep it in `.env` only. |
| `RUST_LOG` | No | Log filter (tracing `EnvFilter` syntax). Unset, the console default is `info,jarvis_api=debug,jarvis_bridge_client=debug`, and the Windows service default is `info,jarvis_api=debug,jarvis_bridge_client=info`. |
| `JARVIS_CONFIG` | No | Path to the config file, used when `--config` is not given. |

`.env` is read from the working directory by `dotenvy` at startup. Variables already set in the environment
take precedence over the file.

Generate a signing secret with a cryptographically secure source:

```powershell
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
$bytes = New-Object byte[] 48
$rng.GetBytes($bytes)
[Convert]::ToBase64String($bytes)
```

The result is 64 characters, which is above the 32-byte minimum, and random enough to pass the other checks. Paste it after `JARVIS_JWT_SECRET=` in `.env`.

### Variables used only by tests and tools

| Variable | Used by | Effect |
|---|---|---|
| `JARVIS_BRIDGE_ENDPOINT` | `bridge-client` live tests | Endpoint of the EA. Default `tcp://127.0.0.1:5555`. |
| `JARVIS_BRIDGE_KEY`, `JARVIS_BRIDGE_KEY_FILE` | `bridge-client` live tests and `bridge_cli` | The shared key (hex), or the path of `bridge.key`. The live tests need one of them. `bridge_cli` also has `--key-file`. |
| `JARVIS_MOCK_KEY_FILE` | `mock_server` example | Where the offline mock writes its random key when `JARVIS_BRIDGE_KEY` is not set. Default `target/mock-bridge.key`. |
| `JARVIS_LIVE_SYMBOL` | `bridge-client` live tests | Broker symbol the tests read or trade, default `EURUSD`. Use your broker's name. |
| `JARVIS_OLLAMA_URL`, `JARVIS_OLLAMA_MODEL`, `JARVIS_ANTHROPIC_URL`, `JARVIS_ANTHROPIC_MODEL`, `JARVIS_OPENAI_URL`, `JARVIS_OPENAI_MODEL` | `llm` live tests | Override the provider endpoint and model. |
| `LIBCLANG_PATH` | Build of the `whisper` feature | Folder that contains `libclang.dll`. `scripts/check.ps1` sets it when LLVM is installed in its default folder. |

## Generating and checking the example file

The template at `config/jarvis.example.toml` is parsed by a unit test, so an invalid example fails the test
suite (`cargo test --workspace`). Run the full set of checks with `pwsh scripts/check.ps1`.

## Related docs

- [Windows service](windows-service.md) for the `service` commands and the log folder.
- [Runbook](runbook.md) for day-to-day operation.
- [VPS guide](vps.md) for the settings that matter on a server.
