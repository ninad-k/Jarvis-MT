# Arc MT web HUD

SvelteKit 2 / Svelte 5 (runes) single-page app, TypeScript strict, Tailwind 4, a Three.js orb.
The build is static files (`build/`) that the Rust server (`jarvis-api`) serves.

## Commands

```sh
pnpm install
pnpm dev               # Vite dev server, /api proxied to http://127.0.0.1:8787 (incl. WebSocket)
pnpm dev:mock          # no backend needed: every endpoint is answered by src/lib/mock.ts
pnpm check             # svelte-check + service worker type check (must be 0 errors, 0 warnings)
pnpm test              # vitest unit tests
pnpm exec playwright install chromium   # once
pnpm test:e2e          # Playwright smoke tests against dev:mock (Chromium only)
pnpm build             # static build into build/
pnpm icons             # regenerate static/icons/*.png (only when the artwork changes)
```

## What is in it

| Area | Where |
|---|---|
| Confirmation modal (countdown, batch two-stage, results, error states) | `lib/components/ConfirmModal.svelte`, `lib/state/preview.svelte.ts`, `lib/countdown.ts` |
| Positions, per-row Close / Close half, batch Close menu, pending orders | `lib/components/Positions.svelte`, `PendingOrders.svelte`, `lib/state/trade-actions.svelte.ts` |
| Kill switch (engage, TOTP release, banner) | `lib/components/KillSwitch.svelte`, `lib/state/killswitch.svelte.ts` |
| Voice: capture, WAV encode/resample, intent routing, TTS playback | `lib/voice/*`, `lib/state/voice.svelte.ts`, `lib/state/speech.svelte.ts` |
| Login, session, 401 handling | `routes/login`, `lib/state/auth.svelte.ts`, `lib/api.ts` |
| WebSocket events with back-off | `lib/state/events.svelte.ts`, `lib/backoff.ts` |
| Audit log | `routes/audit` |
| PWA: manifest, icons, service worker, offline page | `static/manifest.webmanifest`, `static/icons`, `static/offline.html`, `src/service-worker` |
| Personas: toggle in the top bar, name in the UI, theme as CSS custom properties, orb colours | `lib/components/PersonaToggle.svelte`, `lib/state/persona.svelte.ts`, `lib/theme.ts`, `routes/layout.css`, `lib/orb/OrbScene.ts` |
| Settings page (`/settings`): persona, voice, model, watchlist, aliases, tighten-only limits, unsaved-changes guard | `routes/settings`, `lib/components/settings/*`, `lib/state/settings.svelte.ts`, `lib/settings-rules.ts` |
| Hands-free (wake word): in-browser voice detector, segment upload, routing | `lib/voice/vad-core.js` (AudioWorklet), `lib/voice/listener.ts`, `lib/voice/wake.ts`, `lib/state/handsfree.svelte.ts`, `lib/components/HandsFreeBar.svelte` |

## Safety rules the UI keeps

- Every trade button (row Close, Close half, batch Close, order Cancel, a chat proposal) only asks the
  server for a preview. Nothing executes until the confirmation modal's Confirm (or a spoken
  "confirm") calls `POST /api/confirm/{token}`.
- Enter never confirms. Esc cancels. Tab stays inside the dialog. A batch needs a second Confirm that
  repeats the count.
- Write controls are disabled while the kill switch is engaged, the bridge is disconnected, or the
  server is unreachable.
- The service worker caches the app shell only and never touches `/api/*`.

## Personas and settings

- The assistant's name in the UI (transcript, status text, placeholders) is the active persona's name. The persona
  list comes from `GET /api/personas`, so a new pack in `personas/` shows up without a web change. Two personas
  render as a segmented toggle, more as a dropdown. The toggle is disabled while a confirmation is open.
- Colours: `applyTheme()` writes `--persona-accent`, `--persona-accent-soft`, `--persona-orb-core` and
  `--persona-orb-glow` on `<html>`; `layout.css` registers them as `<color>` so the change animates (400 ms, instant
  under reduced motion) and maps `--color-accent` / `--color-accent-dim` onto them. Values are validated as
  `#rrggbb` and the accent is lifted to 4.5:1 contrast if a pack is too dark. The red LIVE banner, the red batch
  confirmation and the error orb do not follow the persona.
- Hands-free: set Voice, Input mode to "Hands-free (wake word)", then press Start listening on the main screen. The
  detector runs in an AudioWorklet (speech starts after 60 ms of voice, ends after 700 ms of quiet, at least 400 ms of
  speech, at most 8 s). Each segment goes to `POST /api/voice/transcribe?wake=1`; only a segment with a wake phrase is
  acted on. It pauses while a reply is read aloud. With a confirmation open it honours "cancel" and
  "<wake phrase>, confirm" only.
- Settings are saved per section. Limits can only be tightened (the server refuses anything looser).

## Keyboard

| Key | Where | Does |
|---|---|---|
| Space (hold) | anywhere outside a text box or a button | push-to-talk |
| Esc | confirmation modal | cancel the confirmation |
| Esc | while recording | discard the recording |
| Enter / Shift+Enter | message box | send / new line |
| Arrow keys, Home, End, Esc | menus | move, jump, close |

## Mock mode

`pnpm dev:mock` serves a simulated market and the whole trading API. Switches go in the URL as a
comma-separated list and are remembered for the browser session (`?mock=reset` clears them):

| Switch | Effect |
|---|---|
| `live` | LIVE mode and a real-account badge |
| `bridge` | API up, MT5 bridge down |
| `offline` | API unreachable |
| `auth` | sign-in required: any username and password, TOTP `123456` |
| `novoice` | speech-to-text and text-to-speech reported as unavailable |
| `unknown` | every execute ends in `outcome_unknown` |
| `ttl=4` | confirmation window of 4 s instead of 15 s |
| `settingsfail` | saving settings fails (shows the optimistic rollback) |
| `staleconfig` | the limits page is told a looser config than the gate enforces (shows a server 422 inline) |

Things to type: `close gold`, `close all`, `close buys`, `close 884201552`, `buy 0.1 gold sl 2300 tp 2420`,
`cancel my pending orders`, `buy 5 gold` (rejected by the gate). Closing the NAS100 position fails with
"Market is closed" to show a partial result. `window.__jarvisMock.say("confirm")` sets what the mock
transcriber returns for the next recording. In hands-free mode (`?wake=1`) only scripted speech counts, for example
`say("hey friday what's gold at")`; anything else is ignored background speech. Personas, settings and limit overrides
are remembered for the browser session; the limits code is `123456` when `auth` is on.

## Server contract assumptions

See `docs/api.md`. Where it is silent the client assumes:

- `GET /api/orders` returns `{orders: [...]}` (bridge `get_orders` shape). The pending-orders section
  stays hidden if the server answers 404.
- A confirm failure other than `rate_limited` ends the confirmation; the user asks again.
- A client timeout or dropped connection during `POST /api/confirm` is reported as "Outcome unknown".
- `/api/auth/me` is optional: any non-401 failure is read as "no session needed".
