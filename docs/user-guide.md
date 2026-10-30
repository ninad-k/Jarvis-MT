# User guide

This guide is for traders who use Jarvis MT with MetaTrader 5 (MT5). It explains what you can ask, how confirmations work, and what the safety settings do. For installation, see the [Windows setup](../README.md#windows-setup) in the README.

**Release labels.** Features marked **(available from v0.2.0)** are planned and are not in the current release, v0.1.0. Everything else on this page works in v0.1.0.

## Start on a demo account

Use a demo account. Jarvis MT is built and tested on demo accounts first. The API checks the account type at startup and refuses to start in demo mode when MT5 is logged into a real account. Read [DEMO and LIVE](#demo-and-live) before you change anything.

## What Jarvis can do

In **v0.1.0**, Jarvis reads data from MT5 and answers your questions:

- The current bid and ask price of a symbol.
- Your account summary: balance, equity, floating profit, margin, free margin, margin level and leverage. Your login number, account name and server are never shown.
- Your open positions, optionally for one symbol.
- Your pending orders.
- Your closed trades between two dates, with totals (net profit, commission and swap, overall and per symbol). Jarvis computes the totals itself, so the numbers match the sums of the deals.
- A watchlist of prices in the web app.

In v0.1.0 Jarvis **cannot place, change or close trades**. If you ask it to buy, it tells you it cannot place trades yet.

**(available from v0.2.0)** Jarvis can also propose orders, changes and closes, and you confirm each one before it is sent. It adds voice input and spoken answers, and a login with a password and an authenticator code.

## Questions you can ask (v0.1.0)

| You say | What Jarvis does |
|---|---|
| "what's gold at?" | Shows the bid and ask for gold (XAUUSD by default) |
| "what's nasdaq at?" | Shows the price of the Nasdaq symbol |
| "what's my equity?" | Shows the account summary |
| "show my positions" | Lists open positions |
| "show my pending orders" | Lists pending orders |
| "what did I trade yesterday?" | Lists yesterday's closed trades, with totals |
| "what's my P/L this week?" | Shows the week's closed trades and their totals |

Hinglish works for questions too. The chat model reads these phrases, but results depend on the model you choose, so check the answer against the MT5 terminal.

| You say | Meaning |
|---|---|
| "gold ka price kya hai?" | What is the gold price? |
| "mere open positions dikhao" | Show my open positions |
| "equity kitni hai?" | What is my equity? |
| "kal kya trade kiya tha?" | What did I trade yesterday? |

Always check numbers in the MT5 Trade and History tabs. Look at the date range in the answer too.

## Trading phrases (available from v0.2.0)

Examples in English, with Hinglish alongside:

| Action | English | Hinglish |
|---|---|---|
| Open a trade | "buy 0.1 gold with SL 2300" | "0.1 gold kharido, SL 2300 ke saath" |
| Open a sell | "sell 0.05 nasdaq with TP 18000" | "0.05 nasdaq becho, TP 18000" |
| Change stop loss | "move gold stop loss to 2380" | "gold ka SL 2380 kar do" |
| Close one position | "close my gold position" | "gold ki position band karo" |
| Close a batch | "close all nasdaq longs" | "saari nasdaq longs band karo" |
| Cancel a pending order | "cancel my EURUSD pending order" | "EURUSD ka pending order cancel karo" |

If the symbol, side or volume is unclear, Jarvis asks you. It does not guess.

## How confirmation works (available from v0.2.0)

1. **You ask.** Jarvis makes a **preview**. It shows what it will do, the symbol, the volume, the affected tickets, the estimated profit or loss (at your stop loss and take profit for a new order), and any warnings. **Nothing is sent to MT5 yet.**
2. **You confirm within 15 seconds.** Click **Confirm**, or say "confirm". Each confirmation works once. After 15 seconds the preview expires, and you must ask again.
3. **Or cancel.** Click **Cancel**, or say "cancel".
4. **Batch actions need two confirmations.** A batch is a request that affects several positions or orders, such as "close all" or "close all nasdaq longs". The first confirmation does not run it. Jarvis then asks you to confirm again and repeats the count, for example "Confirm again to close 3 positions". The second confirmation has its own 15 seconds. Cancelling either one cancels both.
5. **Voice timing.** When Jarvis reads a preview aloud, the 15 seconds start when the reading finishes. The window never runs past 30 seconds after the preview was made.
6. **Every step is logged** in the audit log on your PC.

If MT5 rejects an order, Jarvis shows the reason with the MT5 return code.

If a trade request times out after it was sent, Jarvis does **not** retry it. It tells you the outcome is unknown and refreshes your positions. **Check MT5 before you do anything else.** Do not send the same trade again until you know whether the first one went through.

## Kill switch (available from v0.2.0)

The kill switch blocks every write at once: new orders, closes, changes and cancellations. You can switch it on in the web app or through the API.

- Turning it on cancels all open previews immediately.
- It does not close your positions. Use MT5 to close any position you need to close.
- Turning it off needs your authenticator code when login is on.

## DEMO and LIVE

- **DEMO** is the default. Trades go to the demo account in MT5.
- **LIVE** trades with real money. To use LIVE, you must set `mode = "live"` in `config/jarvis.toml`, log MT5 into a real account, and turn on the EA's `InpAllowReal` input. The web app then shows a LIVE banner (planned).
- The API refuses to start in DEMO mode when MT5 is logged into a real account. If the account changes to a real one while Jarvis runs, chat returns `409 mode_mismatch`. Log MT5 back into your demo account to fix it. Do not change `mode` to get past this error.

We recommend testing the full flow on a demo account before you consider LIVE. Jarvis is provided as-is, and you are responsible for any trade you confirm.

## Limits (available from v0.2.0)

Limits are set in the `[limits]` section of `config/jarvis.toml`. Jarvis checks them before it shows a preview, so a preview that breaks a limit is never offered. You see the reason instead.

| Limit | Example setting | What it blocks |
|---|---|---|
| `max_lot` | `1.0` | A new order larger than this volume (closing positions is never limited by it) |
| `max_positions` | `5` | A new order that would push open positions plus pending orders above this count |
| `max_daily_loss` | `500.0` (account currency) | New orders once today's realised and floating loss reaches this amount. Today starts at server midnight. |
| `allowed_symbols` | `["XAUUSD", "EURUSD"]` | New orders on any other symbol. An empty list blocks all new orders. |
| `trading_hours` | days, `start` and `end` in server time | New orders outside the window |

Closing, changing and cancelling stay allowed when the daily-loss or trading-hours limit is reached, because those actions reduce risk. The kill switch blocks them too.

The limits are checked twice for a new order: when the preview is made and again when you confirm. If something changed in between (for example another preview was confirmed and the position limit is now full), the order is refused with a message that says why, nothing is sent, and you ask for it again.

## Symbol names and aliases

You can say "gold", "nasdaq", "dow", "dow jones", "s&p", "bitcoin", "cable" or "fiber". The `[symbols.aliases]` section of the config maps each name to a symbol at your broker. Matching ignores case.

Broker symbol names differ. You may see `XAUUSD`, `XAUUSD.m`, `GOLD`, `USTEC`, `US100` or `NAS100`. To match your broker:

1. Find the exact names in MT5 Market Watch (press Ctrl+M).
2. Set `suffix` under `[symbols]` if your broker adds one, such as `.m`.
3. Set the aliases to the names you want to say.
4. Restart the API.

If a name does not match a symbol, Jarvis reports "unknown symbol". It does not pick a different symbol.

## Privacy

- **Your MT5 login and password stay in MT5.** Jarvis never sends your login number, account name or server to the LLM or to the browser.
- **API keys stay in `.env`**, not in the config file.
- **Where your data goes depends on the LLM provider.** Your messages and the answers from MT5 (prices, positions, deals) are sent to the provider you use. With a cloud provider (Anthropic or OpenAI), that data leaves your PC. With the default local provider (Ollama), it stays on your PC.
- **The audit log is a local file**, `data/jarvis.db`. It records your messages and trade data, so keep it private.
- **The API listens only on your PC** (`127.0.0.1`) in v0.1.0. Login is available from v0.2.0.

## Troubleshooting

| What you see | What to do |
|---|---|
| Bridge shows as disconnected, or chat says MT5 is not reachable | Start MT5, make sure JarvisBridge is attached to a chart, and check the Experts tab for the "listening" line. Jarvis retries on each question, so no restart is needed. |
| `409 mode_mismatch` | MT5 is logged into a real account. Log it into your demo account. |
| "unknown symbol" | Fix the alias or the suffix in the config, then restart the API. |
| The first answer takes about 100 seconds | The local model is loading. Later answers are much faster. |
| `502 llm_error` | With the local provider, start Ollama and check the model is pulled. With a cloud provider, check the key in `.env`. |
| Numbers look wrong | Compare them with MT5. Ask again with a narrower date range. If they still differ, report it. |
| "outcome unknown" after a trade (available from v0.2.0) | Do not resend the trade. Check MT5 positions and orders first. |

For more detail, see the [runbook](runbook.md#troubleshooting).
