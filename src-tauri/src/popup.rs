//! Result popup. The window is created hidden at startup and reused.
//!
//! Flow: Rust emits a `popup` event -> JS renders and measures -> JS calls `popup_fit`
//! -> Rust sizes, positions and shows the window. One path for every state.

use crate::translate::Translation;
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window};

const CURSOR_GAP: f64 = 12.0; // logical px
const MAX_HEIGHT_RATIO: f64 = 0.6; // of the monitor work area; longer text scrolls

/// Id of the newest hotkey press. Results of older presses are dropped.
static LATEST: AtomicU64 = AtomicU64::new(0);
/// Cursor position (physical px) at the newest hotkey press.
static ANCHOR: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));

#[derive(Serialize, Clone)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Payload {
    Loading { source: String },
    Done { source: String, #[serde(flatten)] result: Translation },
    Error { source: String, message: String },
}

#[derive(Serialize, Clone)]
struct Msg {
    id: u64,
    #[serde(flatten)]
    payload: Payload,
}

/// Starts a new request: remembers where the cursor is. Call when the hotkey fires.
pub fn begin(app: &AppHandle) -> u64 {
    if let Ok(p) = app.cursor_position() {
        *ANCHOR.lock().unwrap() = (p.x, p.y);
    }
    LATEST.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn update(app: &AppHandle, id: u64, payload: Payload) {
    if id == LATEST.load(Ordering::SeqCst) {
        let _ = app.emit_to("popup", "popup", Msg { id, payload });
    }
}

#[tauri::command]
pub fn popup_fit(app: AppHandle, id: u64, width: f64, height: f64) {
    if id != LATEST.load(Ordering::SeqCst) {
        return;
    }
    let Some(win) = app.get_webview_window("popup") else { return };
    let anchor = *ANCHOR.lock().unwrap();
    let _ = place(&win, anchor, width, height);
    let _ = win.show();
    // Allowed: Windows lets the process that received the hotkey take the foreground.
    let _ = win.set_focus();
}

#[tauri::command]
pub fn popup_hide(app: AppHandle) {
    if let Some(win) = app.get_webview_window("popup") {
        let _ = win.hide();
    }
}

/// tao reports focus loss whenever keyboard focus leaves the window's own handle, which
/// also happens inside the popup (webview, header drag). Hide only if another window
/// really became the foreground window.
pub fn hide_if_inactive(win: Window) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let Ok(own) = win.hwnd() else { return };
        if unsafe { GetForegroundWindow() }.0 as isize != own.0 as isize {
            let _ = win.hide();
        }
    });
}

/// Below-right of the cursor; flips above when there is no room below; stays on the
/// cursor's monitor. Size comes from the page (CSS px): short text, small popup.
fn place(win: &WebviewWindow, (cx, cy): (f64, f64), css_width: f64, css_height: f64) -> tauri::Result<()> {
    let Some(mon) = win.monitor_from_point(cx, cy)?.or(win.primary_monitor()?) else {
        return Ok(());
    };
    let s = mon.scale_factor();
    let area = mon.work_area();
    let (ax, ay) = (area.position.x, area.position.y);
    let (aw, ah) = (area.size.width as i32, area.size.height as i32);
    let w = ((css_width * s).ceil() as i32).min(aw);
    let h = ((css_height * s).ceil() as i32).min((ah as f64 * MAX_HEIGHT_RATIO) as i32);
    let gap = (CURSOR_GAP * s) as i32;
    let (cx, cy) = (cx as i32, cy as i32);

    let x = (cx + gap).min(ax + aw - w).max(ax);
    let y = if cy + gap + h <= ay + ah { cy + gap } else { (cy - gap - h).max(ay) };
    // Position first: moving to a monitor with another DPI may rescale the window.
    win.set_position(PhysicalPosition::new(x, y))?;
    win.set_size(PhysicalSize::new(w as u32, h as u32))?;
    Ok(())
}
