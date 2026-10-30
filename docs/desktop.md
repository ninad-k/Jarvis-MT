# Desktop app (Tauri 2)

`apps/desktop` is a small native shell around the Jarvis MT web UI. It gives you a normal
Windows application (installer, Start menu entry, its own window) without a browser tab.
It contains no trading logic. All trading, auth and the action gate stay in `jarvis-api`
(ADR 0015).

- Tauri 2.12.5 (`tauri-cli` 2.12.4), Rust 2024 edition, WebView2 on Windows.
- Identifier: `com.ninadk.jarvismt`. Product name: `Jarvis MT`.
- Plugins: `store` (saves the backend URL), `single-instance`, `window-state` (desktop only).

## How it works

The window opens a tiny bundled **launcher page** (`apps/desktop/ui`, plain HTML, CSS and JS,
no framework). The launcher asks the Rust side to connect to the saved backend URL
(default `https://127.0.0.1:8787`). Rust checks `GET /api/health`, and if the server answers
like Jarvis MT the window navigates to that origin. `jarvis-api` serves the SPA there
(`[server] static_dir = "web/build"`), so the UI loads from the backend itself.

If the server is down, or its certificate is not trusted, the launcher stays open, shows the
reason and lets you fix the URL. Menu `Jarvis MT > Backend settings...` (Ctrl+,) returns to
that page at any time. `Reload` is Ctrl+R.

### Why the UI is loaded from the backend

The SPA calls relative URLs (`/api/...`) and authenticates with an `HttpOnly`, `Secure`,
`SameSite=Strict` cookie. Three ways to use it from a desktop shell:

| Option | Result |
|---|---|
| Bundle `web/build` in the app and set an API base URL | Needs changes in `web/` (every `fetch` and the WebSocket). The page origin becomes `tauri.localhost`, so the API has to allow CORS with credentials and a `SameSite=None` cookie. That weakens the cookie model. |
| Proxy `/api` from the shell | A local proxy process inside the app. More code, more attack surface. |
| **Load the SPA from the backend origin (chosen)** | No change in `web/`. The UI and API are same-origin, so the cookie stays `SameSite=Strict`, the API's own CSP applies to the UI, and no CORS is involved. |

The cost is that the app needs a reachable backend to show anything. That is also true of the
bundled option (the UI is useless without the API), so nothing is lost. The bundled launcher
is what makes the failure readable.

Because the main app runs from the backend origin, the desktop build is always the same UI
version as the server. There is nothing to keep in sync.

## Prerequisites (Windows)

- Rust (stable, MSVC toolchain) and the Visual Studio Build Tools "Desktop development with C++".
- Node 22 or newer and pnpm.
- WebView2 runtime (included in Windows 11; the installer downloads it on Windows 10).
- The first bundle downloads NSIS and WiX from the Tauri binary releases; nothing is installed globally.

## Commands

All from the repository root.

```powershell
pnpm -C apps/desktop install

# Run against a running backend (jarvis-api with static_dir set), opens the launcher
pnpm -C apps/desktop dev

# Run against the Vite dev server instead (starts `pnpm -C web dev` for you; needs the API on 8787)
pnpm -C apps/desktop dev:web

# Debug bundle, quick to build
pnpm -C apps/desktop build:debug

# Release installers
pnpm -C apps/desktop build

# Rust checks
cargo test   --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
```

Output goes to `apps/desktop/src-tauri/target/<debug|release>/bundle/`:

| Artifact | Release size (v0.1.0) |
|---|---|
| `nsis/Jarvis MT_0.1.0_x64-setup.exe` | 1.75 MiB |
| `msi/Jarvis MT_0.1.0_x64_en-US.msi` | 2.45 MiB |
| `release/jarvis-desktop.exe` (portable, needs WebView2) | 4.8 MiB |

`apps/desktop/src-tauri` is **not** part of the root Cargo workspace (it is listed under
`exclude`) and has its own `Cargo.lock`. `cargo build --workspace` at the root never builds it.

The `build` and `dev` scripts do not build `web/`: the shell bundles only the launcher.
Build the SPA with `pnpm -C web build` for the backend to serve.

### Icons

`assets/icon-source.png` (1024 px orb) is drawn by `scripts/make-icon.mjs`. Regenerate every
size, including the Android and iOS sets, with:

```powershell
pnpm -C apps/desktop icons
```

## Configuration

| What | Where |
|---|---|
| Backend URL | `%APPDATA%\com.ninadk.jarvismt\settings.json`, key `backend_url`. Set it from the launcher page. |
| Window size and position | `%APPDATA%\com.ninadk.jarvismt\.window-state.json` |
| Product, identifier, bundle targets | `apps/desktop/src-tauri/tauri.conf.json` |

Accepted backend URLs: `https://host[:port]`, or `http://` on `localhost`, `127.0.0.1` or `[::1]`
only. Paths, query strings, credentials and other schemes are rejected or stripped.

### Backend setup

In `config/jarvis.toml`:

```toml
[server]
bind = "127.0.0.1:8787"
static_dir = "web/build"
tls_cert = "certs/jarvis.crt"
tls_key  = "certs/jarvis.key"
```

No CORS entry is needed for the desktop app in this design, because the page and the API share
an origin. (`[server] dev_origins` is empty by default and must stay empty for the desktop app.)

### Trusted local certificate (mkcert)

The app does **not** offer an option to skip certificate checks. Create a certificate your
computer trusts instead. `mkcert` installs a private root CA in the Windows trust store, which
both the launcher's health check and WebView2 use.

```powershell
winget install FiloSottile.mkcert
mkcert -install
mkdir certs
mkcert -cert-file certs/jarvis.crt -key-file certs/jarvis.key 127.0.0.1 localhost ::1
```

`certs/`, `*.pem` and `*.key` are git-ignored. Keep the mkcert root CA key (`mkcert -CAROOT`)
private: anyone holding it can issue certificates your computer trusts. For a VPS, use a normal
public certificate (for example Let's Encrypt) and put its `https://` address in the launcher.

## Security model

- **No IPC for the app itself.** The capability file (`capabilities/default.json`) grants the
  `main` window two custom commands, `get_backend_url` and `connect`, and nothing else: no
  shell, no filesystem, no `core:default`. It lists no `remote` URLs, so once the window shows
  the backend origin the page cannot call any command. This was checked: from the backend
  page `get_backend_url` is refused with "not allowed on window main".
- **Navigation lock.** The window may only navigate to the bundled pages and the configured
  backend origin (exact scheme, host and port). Links to anything else are refused. `window.open`
  and new windows are denied. Unit tests cover the rules and a manual check confirmed an
  external `location.href` stays on the backend page.
- **Strict CSP on the bundled page:** `default-src 'none'; script-src 'self'; style-src 'self';
  img-src 'self'; connect-src ipc: http://ipc.localhost; base-uri 'none'; form-action 'none';
  frame-ancestors 'none'; object-src 'none'`. The launcher has no inline script or style.
- **The backend page's CSP comes from `jarvis-api`**, not from the shell. It should send a CSP
  that allows its own origin for `connect-src` (including `wss:` for `/api/ws`), because Tauri
  does not inject one into remote pages.
- **Cookies stay in WebView2** with `HttpOnly`, `Secure` and `SameSite=Strict`. The shell never
  reads or stores credentials. It stores only the backend URL.
- **TLS:** HTTPS is required except for loopback. The health check uses the platform trust store.
  There is no insecure mode.
- **Release builds** have devtools disabled (the Tauri `devtools` feature is not enabled and the
  window sets `devtools(false)` outside debug builds). The asset protocol is off and
  `freezePrototype` is on.
- **Single instance:** a second launch focuses the existing window.

## Signing and SmartScreen

The installers are unsigned. Windows SmartScreen shows "Windows protected your PC" for unsigned
downloads, and the publisher shows as unknown. Choose "More info > Run anyway" for your own
builds. To remove the warning, sign the binaries and installers with an Authenticode certificate
(an EV certificate or Azure Trusted Signing builds reputation fastest):

```json
{
  "bundle": {
    "windows": {
      "certificateThumbprint": "<thumbprint of the cert in the Windows store>",
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com"
    }
  }
}
```

For a cloud signing service use `bundle.windows.signCommand` instead. Keep the certificate and
any signing credentials out of the repository; supply them from CI secrets. Auto-update is not
included. Adding it later needs a signing key pair and `tauri-plugin-updater`.

## Mobile (Android and iOS)

The project is configured for mobile but has **not** been built: this repository's Windows
machine has no Android or iOS SDK, and `tauri android init` / `ios init` were not run.
ADR 0015 treats the PWA as the supported phone client; use the desktop shell's mobile
targets only if you need an installable app.

What is already in place: the `com.ninadk.jarvismt` identifier, `bundle.android` (`minSdkVersion` 24)
and `bundle.iOS` (`minimumSystemVersion` 14.0) in `tauri.conf.json`, the mobile entry point in
`lib.rs` (`#[cfg_attr(mobile, tauri::mobile_entry_point)]`), the `staticlib`/`cdylib` crate types,
desktop-only plugins and menu behind `cfg(desktop)`, and the Android and iOS icon sets.

### Android

Prerequisites: Android Studio with SDK Platform, SDK Platform-Tools, NDK (Side by side), Build-Tools
and Command-line Tools; JDK 17 (Android Studio's JBR works); then set `JAVA_HOME`, `ANDROID_HOME`
and `NDK_HOME`.

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
pnpm -C apps/desktop tauri android init     # generates src-tauri/gen/android (commit it)
pnpm -C apps/desktop tauri android dev      # emulator or USB device
pnpm -C apps/desktop tauri android build    # APK and AAB, needs a signing keystore for release
```

### iOS (macOS only)

Prerequisites: Xcode, an Apple Developer account for device builds, CocoaPods, and your team id in
`bundle.iOS.developmentTeam` (or the `APPLE_DEVELOPMENT_TEAM` variable).

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
pnpm -C apps/desktop tauri ios init         # generates src-tauri/gen/apple (commit it)
pnpm -C apps/desktop tauri ios dev
pnpm -C apps/desktop tauri ios build
```

### Mobile caveats

- A phone cannot reach `127.0.0.1` on your PC. Point the launcher at an `https://` address the
  phone can reach (VPS, or a LAN name). The certificate must be trusted by the phone: use a public
  certificate, or install your mkcert root CA on the device.
- There is no menu on mobile, so there is no "Backend settings" entry. To change the URL, clear the
  app's data. Add an in-launcher control before shipping a mobile build.
- `reqwest` with `rustls-platform-verifier` needs its Android glue to be initialised on Android.
  This is untested. If the health check fails there, that is the first place to look.
- Android 9+ blocks cleartext http; the launcher only accepts https for non-loopback hosts anyway.
