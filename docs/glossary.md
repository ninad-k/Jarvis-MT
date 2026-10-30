# Glossary

Terms used in the Arc MT docs, in alphabetical order. Terms marked "(planned)" are not in v0.1.0.

**Action gate.** The part of the backend (the `gate` crate, planned) that checks limits, builds previews and issues confirmation tokens. It is the only route from a trade request to a write. See [SPEC.md](../SPEC.md).

**ADR (architecture decision record).** A numbered record of one significant decision, kept in [docs/adr/](adr/README.md).

**Alias.** A spoken or typed name, such as "gold", that maps to a broker symbol, such as `XAUUSD`, in `[symbols.aliases]`.

**Argon2id.** The password hashing function used for login (planned).

**Ask.** The price at which you can buy a symbol. See also **Bid**.

**Audit log.** An append-only SQLite table, `audit`, in `data/jarvis.db`. It records each step of a chat turn: your message, tool calls, bridge requests and replies, and the reply.

**Balance.** The cash in the account, excluding floating profit and loss.

**Batch action.** One request that acts on several positions or orders at once, such as "close all". It needs a second confirmation (planned).

**Bid.** The price at which you can sell a symbol. See also **Ask**.

**Bridge.** The link between the Jarvis backend and the `JarvisBridge` EA: ZeroMQ REQ/REP on `tcp://127.0.0.1:5555`. See [docs/protocol.md](protocol.md).

**Confirmation token.** A one-time code issued with a preview. It is valid for 15 seconds and is sent only to the browser. Confirming with it sends the action to MT5 (planned).

**Deal.** One execution record in MT5 history, such as the entry or exit of a trade. The deal history lists deals, not positions.

**Demo account.** An MT5 account with virtual money. Jarvis's default mode. See also **LIVE**.

**DLL import.** An MT5 permission that lets an EA call functions in a DLL. `JarvisBridge` needs it for its own EA only. Do not turn it on for all EAs.

**EA (Expert Advisor).** A program that runs inside MT5 and is attached to a chart. In Arc MT, `JarvisBridge` is the EA.

**est_pnl.** The estimated profit or loss shown in a preview. For a close, it is the floating profit or loss being realised. For a new order, it is the estimate at the stop loss and the take profit (planned). See [ADR-0009](adr/0009-est-pnl-definition.md).

**Equity.** Balance plus floating profit or loss.

**Floating P/L.** Profit or loss on open positions that has not yet been realised.

**Free margin.** Equity minus the margin in use. It is the margin available for new positions.

**Hinglish.** Hindi written in Latin script and mixed with English, as in "gold ka price kya hai?".

**HUD.** The web app's main screen, which shows the account strip, watchlist, positions, chat and bridge status.

**JWT (JSON Web Token).** The signed value stored in the login session cookie (planned).

**Kill switch.** A switch that blocks every write at once: new orders, closes, changes and cancellations (planned). It does not close positions by itself.

**Limit.** A rule in `[limits]` that Jarvis checks before a preview, such as `max_lot` or `max_daily_loss` (planned).

**LIVE.** Trading on a real-money account. Requires `mode = "live"` and the EA's `InpAllowReal` input (planned). See also **DEMO**.

**LLM (large language model).** The model that reads your message, calls the read tools and turns the results into an answer. Jarvis supports Ollama (local), Anthropic and OpenAI.

**Lot.** The unit of trade size. The number of units in one lot depends on the symbol and the broker. See also **Volume**.

**Magic number.** An integer that tags orders and positions with the EA or strategy that placed them. `get_positions` reports it. Jarvis v0.1.0 only reports it.

**Margin.** The part of your balance that MT5 holds as collateral for open positions.

**Margin level.** Equity divided by margin, shown as a percentage. It is `0` when no margin is used.

**Mode mismatch.** The error (`409 mode_mismatch`) returned when the config says demo but MT5 is logged into a real account.

**mql-zmq.** The open-source ZeroMQ binding for MQL5 that `JarvisBridge` uses. It is vendored in `mql5/`. See [THIRD_PARTY.md](../mql5/THIRD_PARTY.md).

**MT5 (MetaTrader 5).** The trading platform from MetaQuotes that runs your broker's accounts and the EA.

**Order.** An instruction to buy or sell. A market order fills at once. A pending order waits for a price.

**Outcome unknown.** The response (`504 outcome_unknown`) when a write request timed out after it was sent. The trade may or may not have executed, so Jarvis never retries it (planned). See [ADR-0013](adr/0013-unknown-outcome-for-timed-out-writes.md).

**Pending order.** A buy or sell limit or stop order that waits until the market reaches its price. Listed by `get_orders`.

**Position.** An open trade, identified by a ticket number. It has a volume, an open price and a current profit or loss.

**Preview.** A description of a proposed action: what it does, which symbols and tickets it affects, the volume, and the estimated profit or loss. It does nothing until you confirm it (planned).

**Propose tool.** A tool the LLM can call to create a preview, such as `propose_order` (planned). It cannot send a trade. See [ADR-0005](adr/0005-llm-read-and-propose-tools-only.md).

**PWA (progressive web app).** A web app that can be installed from the browser and used on a phone (planned).

**Read-back.** Jarvis reading a preview aloud. In voice turns, the confirmation window starts when read-back ends (planned). See [ADR-0007](adr/0007-voice-token-readback-timing.md).

**Read tool.** A tool the LLM can call to get data, such as `get_price` or `get_positions`. Read tools cannot change anything.

**Realised P/L.** Profit or loss from trades that have closed.

**REQ/REP.** A ZeroMQ request-reply pattern. The backend is the request side (REQ) and the EA is the reply side (REP). See [ADR-0002](adr/0002-zeromq-req-rep-bridge.md).

**Retcode.** A numeric result code. For read commands it is the MT5 `GetLastError()` value, such as `4301` for an unknown symbol. For trades it is the MT5 trade return code, such as `10009` for a completed request.

**Server time.** The clock of the broker's trade server. Trading hours, history date ranges and protocol times use it, and it can differ from your local time.

**SL (stop loss).** A price at which an open position closes to limit a loss.

**Spread.** The gap between the bid and the ask price of a symbol. The API reports it in points as `spread_points`.

**STT (speech to text).** Turning your spoken words into text (planned, using whisper.cpp through whisper-rs).

**Suffix.** A text added to every symbol name, such as `.m`, when a broker adds one. Set as `[symbols] suffix`.

**Symbol.** The name of a tradable instrument at your broker, such as `XAUUSD`.

**Tauri.** The framework for the planned desktop app (planned). See [ADR-0015](adr/0015-desktop-tauri-and-pwa.md).

**Ticket.** MT5's unique number for an order, a position or a deal.

**TOTP (time-based one-time password).** The six-digit code from an authenticator app, required with your password at login (planned).

**TP (take profit).** A price at which an open position closes to lock in a profit.

**TTS (text to speech).** Turning text into spoken words, used to read answers and previews aloud (planned, using Piper).

**Volume.** The size of an order or position, in lots. See also **Lot** and **Volume step**.

**Volume step.** The smallest allowed change in volume for a symbol. Volumes are rounded to it before any write to MT5.

**Watchlist.** The list of symbols and prices shown in the web app, set by `[symbols] watchlist`.

**ZeroMQ.** The messaging library that carries requests between the backend and the EA.
