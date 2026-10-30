//! Jarvis MT desktop shell.
//!
//! The window starts on a tiny bundled launcher page (`ui/`). The launcher asks
//! this crate to connect to the configured backend, and on success the window
//! navigates to the backend origin, where jarvis-api serves the SPA. The
//! backend page gets no IPC (see `capabilities/default.json`), and navigation
//! is limited to the bundled pages and the configured backend origin.

mod backend;

use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder, webview::NewWindowResponse};
use tauri_plugin_store::StoreExt;
use url::Url;

const STORE_FILE: &str = "settings.json";
const STORE_KEY: &str = "backend_url";

/// State shared with the navigation guard and the commands.
#[derive(Default)]
struct Shared {
    /// Origin of the configured backend (the only remote origin allowed).
    backend: Mutex<Option<Url>>,
    /// The bundled launcher page, used to get back to the settings.
    home: Mutex<Option<Url>>,
}

fn stored_url(app: &AppHandle) -> Option<Url> {
    let store = app.store(STORE_FILE).ok()?;
    let value = store.get(STORE_KEY)?;
    backend::normalize(value.as_str()?).ok()
}

fn save_url(app: &AppHandle, url: &Url) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(STORE_KEY, url.as_str());
    store.save().map_err(|e| e.to_string())
}

/// The saved backend URL, or the default when none is saved yet.
#[tauri::command]
fn get_backend_url(app: AppHandle) -> String {
    let url = stored_url(&app).map_or_else(|| backend::DEFAULT_URL.to_string(), |u| u.to_string());
    url.trim_end_matches('/').to_string()
}

/// Validates and saves the URL, checks the server answers, then opens it in
/// this window. Returns a readable error otherwise.
#[tauri::command]
async fn connect(app: AppHandle, window: WebviewWindow, url: String) -> Result<(), String> {
    let target = backend::normalize(&url)?;
    save_url(&app, &target)?;
    *app.state::<Shared>()
        .backend
        .lock()
        .map_err(|e| e.to_string())? = Some(target.clone());
    backend::probe(&target).await?;
    window.navigate(target).map_err(|e| e.to_string())
}

#[cfg(desktop)]
fn show_settings(app: &AppHandle) {
    let (Some(window), Some(mut home)) = (
        app.get_webview_window("main"),
        app.state::<Shared>()
            .home
            .lock()
            .ok()
            .and_then(|h| h.clone()),
    ) else {
        return;
    };
    home.set_fragment(Some("settings"));
    let _ = window.navigate(home);
}

#[cfg(desktop)]
fn build_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
    let settings = MenuItem::with_id(
        app,
        "settings",
        "Backend settings...",
        true,
        Some("CmdOrCtrl+,"),
    )?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit"))?;
    let file = Submenu::with_items(app, "Jarvis MT", true, &[&settings, &reload, &sep, &quit])?;
    Menu::with_items(app, &[&file])
}

/// Runs the app. Also the mobile entry point.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        // Must be registered first so a second launch focuses the first window.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
        builder = builder.plugin(tauri_plugin_window_state::Builder::default().build());
    }

    builder
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(Shared::default())
        .invoke_handler(tauri::generate_handler![get_backend_url, connect])
        .setup(|app| {
            let handle = app.handle().clone();
            *app.state::<Shared>().backend.lock().expect("state") = stored_url(&handle);
            let dev = app.config().build.dev_url.clone();

            let cfg = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .expect("main window is defined in tauri.conf.json");
            let guard = handle.clone();
            let window = WebviewWindowBuilder::from_config(app.handle(), cfg)?
                .devtools(cfg!(debug_assertions))
                .on_navigation(move |target| {
                    let backend = guard
                        .state::<Shared>()
                        .backend
                        .lock()
                        .ok()
                        .and_then(|b| b.clone());
                    backend::navigation_allowed(target, backend.as_ref(), dev.as_ref())
                })
                .on_new_window(|_url, _features| NewWindowResponse::Deny)
                .build()?;
            *app.state::<Shared>().home.lock().expect("state") = window.url().ok();

            #[cfg(desktop)]
            {
                app.set_menu(build_menu(&handle)?)?;
                app.on_menu_event(|app, event| match event.id().as_ref() {
                    "settings" => show_settings(app),
                    "reload" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.eval("location.reload()");
                        }
                    }
                    _ => {}
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Jarvis MT");
}
