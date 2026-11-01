#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod popup;
#[cfg(windows)]
mod selection;
mod translate;

use popup::Payload;
use std::{
    sync::{mpsc, Mutex, PoisonError},
    time::Instant,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Windows draws tray icons at 16 px (100% scaling), 20, 24 or 32 px (200%). Each size is
/// rendered on its own (see `npm run icons`); a shrunk large icon looks blurry.
// ponytail: picked once at startup; a display-scaling change needs an app restart.
fn tray_icon(scale: f64) -> Image<'static> {
    let png: &[u8] = match (16.0 * scale).round() as u32 {
        ..=17 => include_bytes!("../icons/tray/16x16.png"),
        18..=21 => include_bytes!("../icons/tray/20x20.png"),
        22..=27 => include_bytes!("../icons/tray/24x24.png"),
        _ => include_bytes!("../icons/tray/32x32.png"),
    };
    Image::from_bytes(png).expect("bundled tray icon")
}

/// From Cargo.toml: the one place for version and homepage.
#[derive(serde::Serialize)]
struct About {
    version: &'static str,
    homepage: &'static str,
}

#[tauri::command]
fn about() -> About {
    About {
        version: env!("CARGO_PKG_VERSION"),
        homepage: env!("CARGO_PKG_HOMEPAGE"),
    }
}

/// Opens the fixed homepage in the default browser; never a URL coming from the page.
#[tauri::command]
fn open_homepage() {
    // ponytail: Windows only; switch to tauri-plugin-opener when other platforms come.
    if let Err(e) = std::process::Command::new("explorer").arg(env!("CARGO_PKG_HOMEPAGE")).spawn() {
        eprintln!("[about] cannot open homepage: {e}");
    }
}

/// Builds a window from its tauri.conf.json entry (all are "create": false). Built in code
/// so every window gets the same WebView2 data folder.
fn build_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    let cfg = app.config().app.windows.iter().find(|w| w.label == label).expect("window config");
    WebviewWindowBuilder::from_config(app, cfg)?
        .data_directory(config::webview_dir(app))
        .build()
}

/// Settings is built on demand and destroyed on close: each open window costs a WebView2
/// renderer process, and settings is rarely open.
fn show_settings(app: &AppHandle) {
    let win = app.get_webview_window("settings").or_else(|| {
        build_window(app, "settings")
            .inspect_err(|e| eprintln!("[settings] cannot open: {e}"))
            .ok()
    });
    if let Some(w) = win {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// One thread for all hotkey presses: owns the UI Automation object and keeps
/// two Ctrl+C captures from overlapping.
fn hotkey_worker(app: AppHandle, presses: mpsc::Receiver<()>) {
    let selector = selection::Selector::new();
    while presses.recv().is_ok() {
        let t0 = Instant::now();
        let id = popup::begin(&app);
        let text = selector.get();
        // Presses queued while capturing would re-capture the same text.
        while presses.try_recv().is_ok() {}
        let Some(text) = text else {
            // Silent on purpose: an accidental press must not steal focus.
            eprintln!("[hotkey] no selection ({:?})", t0.elapsed());
            continue;
        };
        let t_sel = t0.elapsed();
        popup::update(&app, id, Payload::Loading { source: text.clone() });
        let cfg = app.state::<Mutex<config::Config>>().lock().unwrap_or_else(PoisonError::into_inner).clone();
        let translator = app.state::<translate::Translator>().inner().clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let r = translator.translate(&text, &cfg.target_lang, &cfg.fallback_lang).await;
            // Selected and translated text stays out of the log, except in debug builds.
            let outcome = match &r {
                Ok(t) => format!("ok {}->{} ({} chars)", t.src, t.tl, text.chars().count()),
                Err(e) => format!("error: {e}"),
            };
            eprintln!("[hotkey] selection {t_sel:?}, total {:?}: {outcome}", t0.elapsed());
            if cfg!(debug_assertions) {
                eprintln!("[debug] {text:?} => {r:?}");
            }
            let payload = match r {
                Ok(result) => Payload::Done { source: text, result },
                Err(message) => Payload::Error { source: text, message },
            };
            popup::update(&app, id, payload);
        });
    }
}

fn main() {
    let (press_tx, press_rx) = mpsc::channel();
    tauri::Builder::default()
        // Second launch focuses the running instance instead of starting another.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_settings(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |_, _, e| {
                    if e.state == ShortcutState::Pressed {
                        let _ = press_tx.send(());
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            popup::popup_fit,
            popup::popup_hide,
            config::get_settings,
            config::set_settings,
            about,
            open_homepage
        ])
        .setup(move |app| {
            build_window(app.handle(), "popup")?;
            let cfg = config::load(app.handle());
            if let Err(e) = config::apply_autostart(app.handle(), cfg.autostart) {
                eprintln!("[autostart] {e}");
            }
            // Fails when another app (e.g. pot) owns the same hotkey: open settings, which
            // shows the problem and lets the user pick another one.
            if let Err(e) = app.global_shortcut().register(cfg.hotkey.as_str()) {
                eprintln!("[hotkey] cannot register {}: {e}", cfg.hotkey);
                show_settings(app.handle());
            }
            app.manage(Mutex::new(cfg));
            let handle = app.handle().clone();
            std::thread::spawn(move || hotkey_worker(handle, press_rx));

            let translator = translate::Translator::new();
            app.manage(translator.clone());
            tauri::async_runtime::spawn(async move { translator.warm_up().await });

            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::new()
                .icon(tray_icon(app.primary_monitor()?.map_or(1.0, |m| m.scale_factor())))
                .tooltip("Popslate")
                .menu(&menu)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "settings" => show_settings(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|w, e| match e {
            // The popup is pre-created and reused (Alt+F4 must not destroy it): hide instead.
            WindowEvent::CloseRequested { api, .. } if w.label() == "popup" => {
                api.prevent_close();
                let _ = w.hide();
            }
            // Popup closes when the user clicks anywhere else.
            WindowEvent::Focused(false) if w.label() == "popup" => popup::hide_if_inactive(w.clone()),
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running popslate");
}
