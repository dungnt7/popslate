#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod popup;
#[cfg(windows)]
mod selection;
mod translate;

use popup::Payload;
use std::{
    sync::{mpsc, Mutex},
    time::Instant,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WindowEvent,
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

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
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
        let cfg = app.state::<Mutex<config::Config>>().lock().unwrap().clone();
        let translator = app.state::<translate::Translator>().inner().clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let r = translator.translate(&text, &cfg.target_lang, &cfg.fallback_lang).await;
            eprintln!("[hotkey] selection {t_sel:?}, total {:?}: {r:?}", t0.elapsed());
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
        .invoke_handler(tauri::generate_handler![popup::popup_fit, popup::popup_hide])
        .setup(move |app| {
            let cfg = config::load(app.handle());

            // Release only: a dev build must not register target/debug exe to run at login.
            #[cfg(not(debug_assertions))]
            {
                use tauri_plugin_autostart::ManagerExt;
                let launcher = app.autolaunch();
                let _ = if cfg.autostart { launcher.enable() } else { launcher.disable() };
            }
            // Fails when another app (e.g. pot) owns the same hotkey.
            if let Err(e) = app.global_shortcut().register(cfg.hotkey.as_str()) {
                eprintln!("[hotkey] cannot register {}: {e}", cfg.hotkey);
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
            // Windows are pre-created and reused: hide on close, never destroy.
            WindowEvent::CloseRequested { api, .. } => {
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
