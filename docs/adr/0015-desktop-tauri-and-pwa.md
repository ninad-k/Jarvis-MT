# ADR-0015: Desktop app with Tauri 2, and a PWA for phones

- Status: Accepted
- Date: 2026-10-30

## Context

The HUD is a web app served from the API process or from the Vite dev server. Users want an app window on Windows that starts with the PC, and they want to use the HUD from a phone. Mobile builds would need Apple or Android tooling that the project's Windows PC cannot provide.

## Decision

- **Desktop:** a Tauri 2 shell in `apps/desktop/` bundles the built single-page app and talks to the API over HTTPS. Its Rust code is a separate crate with its own lockfile, excluded from the main Cargo workspace because of its heavy dependencies.
- **Phones:** the web app is a progressive web app (PWA) that can be installed from the browser. (Update 2026-10-30: the desktop shell loads the SPA from the backend's own origin, so no CORS entry is needed for it, and CORS is off unless `[server] dev_origins` lists an origin.)
- **Mobile builds:** Tauri mobile targets are configured and documented, but are not built on Windows.

Status: planned for Sprint 5.

## Consequences

- One web codebase serves the browser, the PWA and the desktop app.
- The desktop app depends on the API being reachable over HTTPS. Remote use therefore needs TLS and auth (ADR-0011).
- The desktop build has its own dependencies and toolchain steps, which CI must cover.
- Mobile users get the PWA. Native mobile builds stay out of scope for the Windows build machine, so they are documented rather than built.

## Alternatives considered

- **Electron.** It is mature, but it bundles a full Chromium runtime, which makes the app much larger.
- **PWA only.** It needs no extra build, but it does not give a standalone desktop app that starts with Windows.
- **A native Rust GUI.** It would duplicate the HUD, which is already built in the web stack.
