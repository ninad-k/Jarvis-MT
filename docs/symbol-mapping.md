# Broker symbol mapping

Every broker names the same instrument differently. Gold is `XAUUSD_` at one broker, `GOLD_CASH` at another and `XAUUSD.pr` at a third. The Nasdaq 100 can be `US100_Spot`, `USTEC`, `UT100Roll` or `NDXcash`. A name that works at one broker is "unknown symbol" at the next.

Arc MT therefore speaks one set of names everywhere, the **canonical** names (`XAUUSD`, `US100`, `US30`, `EURUSD`, ...), and maps each one to the broker's own symbol. The map is kept per broker, keyed by the company name that MT5 reports for the account. You normally do nothing: Arc MT reads the broker's full symbol list when it connects, builds the map, and builds it again when you switch to a broker it has not seen in the current run.

This page covers the names, the three map layers, how detection decides, how to review and change a mapping, how to add a broker to the repository, and what to do when a mapping is wrong. The HTTP contract is in [api.md](api.md#broker-symbol-mapping) and the configuration keys are in [configuration.md](configuration.md#symbols).

## Canonical names and aliases

A **canonical name** is the one name Arc MT uses for an instrument. It is upper case, made of letters and digits, and every canonical name is defined in [config/instruments.toml](../config/instruments.toml). Limits (`allowed_symbols`), the watchlist, chat, the gate and the Settings page all use canonical names, so a limit on `US100` applies to whatever the broker calls the Nasdaq 100.

An **alias** is a generic name that means the same instrument, such as `GOLD` or `NAS100`. Aliases resolve to the canonical name before the broker map is consulted. A broker's own name typed in a request (`XAUUSD_`) is also recognised, and resolves to its canonical name too.

Spoken words are a separate list. `gold`, `nasdaq`, `dow` and `cable` are set in `[symbols.aliases]` in `config/jarvis.toml`, and point at a canonical name (`gold = "XAUUSD"`). Matching ignores case.

These are the canonical instruments and their aliases (from `config/instruments.toml`). Each one also has description keywords, such as "nasdaq 100" or "us tech 100", which detection uses on a broker's description text.

| Canonical | Class | Aliases |
|---|---|---|
| XAUUSD | metal | GOLD, XAU |
| XAGUSD | metal | SILVER, XAG |
| XPTUSD | metal | PLATINUM |
| XPDUSD | metal | PALLADIUM |
| US100 | index | NASDAQ, NAS100, NASDAQ100, USTEC, USTECH, NDX, NDQ, NQ100, UT100 |
| US30 | index | DOW, DOWJONES, DJ30, DJI, WS30, WALLSTREET30, US30USD |
| US500 | index | SPX500, SP500, SPX, SPY500, US500USD, USA500 |
| US2000 | index | RUSSELL2000, RUT, US2K |
| GER40 | index | DE40, GER30, DE30, DAX, DAX40, GERMANY40 |
| UK100 | index | FTSE100, FTSE, UKX |
| JP225 | index | JPN225, NIKKEI, NIKKEI225, N225, JAPAN225 |
| EU50 | index | STOXX50, EUSTX50, EURO50, SX5E |
| FRA40 | index | CAC40, FR40, CAC |
| AUS200 | index | ASX200, AU200 |
| HK50 | index | HSI, HANGSENG, HK33 |
| WTI | energy | USOIL, XTIUSD, CRUDE, CL, OILUSD, USCRUDE |
| BRENT | energy | UKOIL, XBRUSD, BRN |
| NATGAS | energy | NGAS, XNGUSD, NG |
| BTCUSD | crypto | BITCOIN, BTC, XBTUSD |
| ETHUSD | crypto | ETHEREUM, ETH |
| LTCUSD | crypto | LITECOIN, LTC |
| XRPUSD | crypto | RIPPLE, XRP |
| EURUSD | fx | EURO, FIBER |
| GBPUSD | fx | CABLE, POUND |
| USDJPY | fx | YEN |
| AUDUSD | fx | AUSSIE |
| USDCAD | fx | LOONIE |
| USDCHF | fx | SWISSY |
| NZDUSD | fx | KIWI |
| EURGBP, EURJPY, GBPJPY, EURCHF, AUDJPY, USDCNH | fx | none |

FX canonical names are six letters. A bare six-letter pair that is not in the table (for example `AUDCAD`) is accepted when both halves are ISO currency codes.

## The three layers

A broker's map is built from three layers. Each layer is a TOML file with one table per broker, keyed by the exact MT5 company name: `[brokers."CFI Financial Markets L.L.C"]`.

| Layer | File | Written by | Ranks |
|---|---|---|---|
| user | `data/broker_symbols.user.toml` | Settings (Save and Reset) | highest |
| curated | `config/broker_symbols.toml` (in the repository) | you, by hand | middle |
| generated | `data/broker_symbols.generated.toml` | detection | lowest |

Each broker table has an optional `note`, a list of `patterns`, and an `explicit` table:

- **explicit**: canonical name (or alias) to the broker's symbol, such as `XAUUSD = "XAUUSD_"`.
- **patterns**: a regular expression matched against the canonical name, and a replacement, such as `{ match = "^([A-Z]{6})$", replace = "${1}.pr" }`. The match must cover the whole name.

### Resolution order

For a canonical name on the current broker, Arc MT does this, in order:

1. **Explicit entry.** The layers are checked from the top: a user entry for the name wins; otherwise a curated one; otherwise a generated one. Within one layer, an entry keyed by the canonical name beats one keyed by an alias (`XAUUSD` beats `GOLD`).
2. **Pattern.** If no layer has an explicit entry, the patterns of all three layers are tried in the order user, curated, generated (file order within a layer). The first full match gives the symbol.
3. **Passthrough.** Otherwise the canonical name is used unchanged.

Explicit entries of every layer are tried before any pattern. So a generated explicit entry for `US100` beats a curated blanket pattern, and a curated pattern does not hide it.

The configured `[symbols] suffix` is a last resort. It is appended to a name only while the current broker has no map in any layer, or no broker is known yet. Leave it empty when the maps work.

After resolution Arc MT checks the name against the broker's catalog. If the broker does not have it, the row is marked **not in this broker's catalog** and the instrument counts as unmapped (see Settings below).

Example, using the curated CFI entries (verified 2026-10-12 from the maintainer's trade history):

| Canonical | Resolves to | Source |
|---|---|---|
| XAUUSD | `XAUUSD_` | curated (explicit) |
| US100 | `US100_Spot` | curated (explicit) |
| EURUSD | `EURUSD_` | pattern (the curated `^(.+)$` rule adds `_`) |
| US100, after the user sets `NAS100.cash` | `NAS100.cash` | user (explicit, beats curated) |

The source names are the ones the Settings page shows: `user`, `curated`, `auto` (generated), `pattern` and `unchanged` (passthrough). A curated pattern is shown as `pattern`.

### Typed and spoken names

When you type or say a name, Arc MT resolves it in this order:

1. A spoken alias from `[symbols.aliases]` (`dow` gives `US30`).
2. A canonical name or generic alias from `config/instruments.toml` (`GOLD` gives `XAUUSD`).
3. A symbol the current broker has, typed in its own spelling (`XAUUSD_`). It is recognised from the broker's catalog and reversed to its canonical name.

The canonical name then goes through the broker's map, and the broker symbol is what reaches MT5.

## When you switch broker or account in MT5

Arc MT notices a change of account the next time it asks the bridge which broker the terminal is logged into. That happens:

- on every status request that reaches the bridge;
- at startup;
- every 30 seconds in the background, even with no browser open;
- before a proposal is checked, and before a confirmation is accepted.

When the company changes:

1. **The map.** If this broker has not been seen in this run, its saved catalog snapshot is loaded (marked stale, and used only to check overrides), and detection runs for it. If the broker was already detected in this run, its current map is used as it stands.
2. **The event.** Arc MT sends `symbol_map_changed` on the WebSocket when it handles the switch, and again when a detection it started finishes, so one switch can send two events. The Settings page refreshes on them.
3. **Previews.** A preview records the company and the trade server it was made on. If the account has changed when you confirm, the confirmation is refused with `409 broker_changed`, the token is cancelled, nothing is sent, and the refusal is written to the audit log. Ask again to get a preview for the new broker. A move between two servers of the same company also refuses an old preview, although the map itself is keyed by company only.
4. **Proposals in progress.** A proposal being prepared while the account changes is either rebuilt for the new broker or refused with `broker_changed`. Nothing is sent. Repeat the request.

Automatic detection is keyed by company, so a change of trade server alone does not start one (and does not change the map).

### Where the files are

| File | Written when | Contents | Safe to delete? |
|---|---|---|---|
| `data/broker_symbols.generated.toml` | every detection (startup, switch, Re-detect) | the detected map of every broker seen | Yes. It is rebuilt by the next detection. Its header says so. |
| `data/broker_symbols.user.toml` | every Save or Reset in Settings | your overrides | No. Deleting it loses your choices. Copy it aside first. |
| `data/broker-catalogs/<slug>.json` | every detection | the broker's full symbol list, the candidates and the summary (`<slug>` is the company name in lower case with dashes, for example `cfi-financial-markets-l-l-c`) | Yes. Re-detect to rebuild it. |

The audit log gets a `symbol_map_detected` row for each detection (with `trigger` `auto` or `manual`) and a `symbol_map_override` row for each override, with the previous value.

## How detection decides

Detection reads a broker's map from its full symbol list. It runs at startup, on a switch to a broker not yet seen in this run, and when you press **Re-detect** in Settings (or call `POST /api/symbols/detect`). Only one detection runs at a time. After a failed automatic attempt for a broker, the next automatic attempt waits 30 seconds. With `[symbols] auto_detect = false` only the manual run happens.

1. **Read the whole catalog.** Every symbol the trade server offers is read, not only the ones in Market Watch, in pages of up to 1000. The broker identity is checked before and after, and if the account changed while the pages were read, the result is discarded.
2. **Find the broker's affix.** The seven FX majors (EURUSD, GBPUSD, USDJPY, AUDUSD, USDCAD, USDCHF, NZDUSD) vote. An affix of up to three characters on one side (a suffix such as `_`, `.pr` or `m`, or a prefix) is accepted when at least four majors carry it. When a broker lists both a bare set and an affixed set (`EURUSD` and `EURUSD_`), the set whose members are more tradable and more often shown in Market Watch wins. A tie goes to the set the metals use, then to the larger set.
3. **Find candidates for each instrument.** A symbol is a candidate when:
   - its name contains the canonical name or an alias as a core, with at most three characters of affix on the outside, or with a tail such as `cash`, `spot`, `roll`, `rolling` or `raw` (`NAS100.cash` is a candidate for `US100` through the alias `NAS100`); or
   - its description contains one of the instrument's keywords, and the symbol is not already a name match for another instrument.

   These are never candidates:
   - disabled symbols;
   - dated futures: a name with a month and year code (`NAS1226`, `GC0826`, `US30Z6`, `NatGas_X6`) or a description that mentions futures together with a month or year;
   - stocks and ETFs: a folder path with stock, equity, share or ETF words (always excluded), or a description with company words such as Inc, Corp, Ltd or plc (kept only when the name is exactly the canonical name);
   - names with a long tail that makes them another product (`XAUUSDINDEX2`).
4. **Score each candidate.** The points are:

   | Signal | Points |
   |---|---|
   | Tradable (full) | +3 |
   | Long-only or short-only trading | +1 |
   | Close-only | -5 |
   | Visible in Market Watch | +3 |
   | Exact canonical name (`US100`) | +4 |
   | Canonical name with an affix (`US100_Spot`) | +3 |
   | Exact alias (`NASDAQ`) | +3 |
   | Alias inside a longer name (`NAS100.cash`) | +2 |
   | Affix equal to the broker's detected affix | +3 |
   | Cash, spot, roll or rolling contract (in the name or the description) | +2 |
   | Description matches a keyword | +3 |
   | Mini or micro variant | -3 |
   | Mentions futures, without a date | -2 |

   Candidates are ranked by score. A tie goes to the shorter affix, then to the name in alphabetical order. Only the top 8 per instrument are kept, each with its score and reasons.
5. **Choose.** Instruments with the strongest best candidate choose first. Each broker symbol can be claimed by one instrument only. The chosen symbol becomes an explicit entry, unless the detected affix pattern would already produce it.
6. **Write the pattern.** When an affix was detected, one generated pattern (`^([A-Z]{6})$`, replaced by the affix around the name) covers the six-letter names that have no explicit entry.
7. **Save and show.** The generated layer and the candidates are saved and shown in Settings.

Detection is deterministic: the same catalog always gives the same map. It is still a check against names, not a check against what each symbol trades. Read the candidates in Settings before you trust a mapping for an instrument you trade.

## Reviewing and changing a mapping in Settings

Open **Settings** and go to **Symbols** (the section is titled "Symbol mapping").

- **Detected broker.** The company and the trade server, the catalog size, when the map was detected, and the detected prefix and suffix. It says "Not detected yet" until the bridge has reported a broker.
- **Re-detect.** Reads the broker's symbols again now. It needs no authenticator code.
- **Export JSON.** Downloads the effective map as `broker-symbols-<slug>.json`. The file holds the company, the server, the time and a `map` from canonical name to broker symbol, listing only the instruments the broker has.
- **Rows.** Instruments are grouped as Metals, Indices, Energy, Crypto, FX and Other. Each row shows the canonical name and its aliases, the broker symbol, a source badge and an **Edit** button. The badges are `user`, `curated`, `auto` (generated), `pattern` and `unchanged`; each has a tooltip.
- **Not in this broker's catalog.** The row's broker symbol is not one the broker has. The same instrument also appears under **Unmapped on this broker**, with a **Map...** button.
- **Attention.** A line under the header says how many instruments need attention. Those are the ones not in the catalog, so the rows and the unmapped list point at the same work.

To change a row, press **Edit**. The search box shows the suggestions (ranked, with their scores and reasons) and searches the whole catalog by name and description. Pick a name, or type one, and press Enter or Save. **Reset to automatic** appears when you have an override and removes it.

With login on, Save and Reset need the current 6-digit code from your authenticator app. A wrong code saves nothing. A name that the broker does not have is refused with `unknown_broker_symbol`.

Your override beats the curated and generated maps for that broker until you reset it. The map is kept per broker company, so an override for one broker does not affect another.

The same actions are available through the API: `GET /api/symbols`, `PUT /api/symbols/map` (with `totp` when login is on), `POST /api/symbols/detect`, `GET /api/symbols/catalog?q=` and `GET /api/symbols/export`. See [api.md](api.md#broker-symbol-mapping). The `/api/symbols/export` route names its file `arc-mt-symbols-<slug>.json`.

## Adding a broker to the repository

Use the curated file, `config/broker_symbols.toml`, for a broker whose names you have checked yourself. A curated map is read at startup and sits above the detected map, so it stays in force until you change it.

1. **Get the exact company name.** The Detected broker line in Settings shows it before the `·`, or read `broker.company` from `GET /api/symbols`. Match it character for character, punctuation included.
2. **Get the broker's own names.** Search the catalog in Settings, or read the export. Check the trade mode and whether the symbol is visible in Market Watch.
3. **Choose the rules.** Use a pattern for the usual affix on six-letter names, and explicit entries for everything else, such as indices. Explicit entries beat patterns in every layer, so list each exception. A pattern also covers every name you did not list. A rule as broad as `^(.+)$` sends every unlisted instrument to a name the broker may not have, and those rows show as not in the catalog.
4. **Write the block.** For example, for a placeholder broker:

   ```toml
   [brokers."Example Markets Ltd"]
   note = "Placeholder example: replace the company name and the symbol names with the broker's own, and say how they were checked."
   patterns = [{ match = "^([A-Z]{6})$", replace = "${1}.r" }]

   [brokers."Example Markets Ltd".explicit]
   XAUUSD = "GOLD.r"
   US30 = "US30.cash"
   US100 = "NAS100.cash"
   ```

   The `note` is free text: record the account and the date, and say whether the names were verified on a real account. Replacements may use `$1` or `${1}` (or `\1`). A pattern must compile, and every group it uses must exist.
5. **Restart Arc MT.** The curated file is read at startup only.
6. **Check the result.** Rows that come from your explicit entries show the `curated` badge, and rows that come from your pattern show `pattern`. None should be marked not in the catalog. Fix any row that is wrong with an override, or change the file and restart.

## Troubleshooting

| What you see | Likely cause | What to do |
|---|---|---|
| A row says "Not in this broker's catalog", or the instrument is under "Unmapped on this broker" | The resolved name is not in the broker's symbol list. The broker may use another name, or may not offer the instrument. | Press Edit and search the catalog for the instrument (for example "gold" or "nasdaq"). Pick the symbol the broker has. If nothing fits, the broker does not offer that instrument, and orders in it are refused. |
| An order or price request is refused with "unknown symbol" and "The broker has no symbol matching ..." | The instrument maps to a name the broker lacks, so the EA answers unknown symbol (retcode 4301) and nothing is sent. | As above: find the right symbol and save an override. |
| A row maps to the wrong symbol (a mini, an expired future, the wrong listing) | Detection chose a candidate whose score is misleading, or a curated rule is too broad. | Open Edit, read the candidates' reasons, and choose the correct name. An override beats every other layer until you reset it. If the curated file is the cause, correct it there too, and restart. |
| The broker added symbols, and some instruments are still unmapped | The map is from the last detection. | Press **Re-detect**. A restart also detects again, because the saved snapshot is treated as stale at startup. |
| "Not detected yet" in Settings, or the map is empty | No broker is known, usually because the EA is not attached or the bridge is down. | Attach JarvisBridge in MT5 (see the [runbook](runbook.md)). Detection runs on the next status request or the next 30 s check. |
| The log says "symbol maps are not loaded" at startup | A layer file is malformed or unreadable, or the instruments file or the curated file is missing. Arc MT still starts, but all maps are off: names pass through with the configured suffix, and detection does not run. | Read the file and the TOML error named in the message, fix it, and restart. Re-detect fails with `symbols_unavailable` while the maps are off. |
| The same warning names `data/broker_symbols.generated.toml` | The generated file is corrupt. The layers load together, so one bad layer turns all maps off. | Stop Arc MT, delete `data/broker_symbols.generated.toml`, and start again. Detection then rebuilds it. |
| The same error for `data/broker_symbols.user.toml` | The user file is corrupt. | Stop Arc MT and copy the file aside. Fix the TOML if you can, or delete it and set the overrides again. Start again. Deleting it loses the overrides. |
| The catalog search in Settings is empty, and the log warns about `data/broker-catalogs/<slug>.json` | The saved catalog snapshot is unreadable. Arc MT ignores it and keeps going. | Delete the file and press **Re-detect**. |
| A symbol you can see in MT5 is missing from the Edit suggestions | The catalog is the one from the last detection, so a symbol the broker added since then is not in it. | Press **Re-detect**, then search again. |

The Exness entries in the curated file are not verified against a real account (see the note in that file). Their explicit names for XAUUSD, US30 and BTCUSD are curated explicit entries, so they stay in force even when detection finds another symbol. For the other six-letter names, the curated `m` rule is a pattern and applies before the detected affix, although a detected explicit name still beats it. Check each Exness row when you first connect, and override any that are wrong.

For logs, see the [runbook](runbook.md) and the "Files" rows in [architecture.md](architecture.md#data-stores).
