//! Get the text selected in the foreground app (Windows).
//!
//! 1. UI Automation: ask the app for its selection. Clipboard untouched.
//! 2. Fallback: simulate Ctrl+C, read the clipboard, then restore the old clipboard.

use std::{
    thread::sleep,
    time::{Duration, Instant},
};
use windows::Win32::{
    System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
    System::DataExchange::GetClipboardSequenceNumber,
    UI::Accessibility::{CUIAutomation8, IUIAutomation2, IUIAutomationTextPattern, UIA_TextPatternId},
    UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_LWIN,
        VK_MENU, VK_RWIN, VK_SHIFT,
    },
    UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow},
};

const MODIFIERS: [VIRTUAL_KEY; 5] = [VK_MENU, VK_CONTROL, VK_SHIFT, VK_LWIN, VK_RWIN];
// Unassigned virtual key. Pressing it while Alt/Win is held stops the app from treating the
// later Alt release as "open menu bar" (or Win release as "open Start").
const VK_MASK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
// In terminals Ctrl+C without a selection kills the running command: never send it there.
const NO_CTRL_C_CLASSES: [&str; 2] = ["CASCADIA_HOSTING_WINDOW_CLASS", "ConsoleWindowClass"];

/// Holds a COM object: create and use on one dedicated thread.
pub struct Selector {
    /// None when UI Automation is unavailable: every capture then uses the Ctrl+C fallback.
    uia: Option<IUIAutomation2>,
}

impl Selector {
    /// Never fails: a missing UI Automation must not take the hotkey down with it.
    pub fn new() -> Self {
        // SAFETY: first COM call on this thread; S_FALSE (already initialized) is fine too.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // SAFETY: COM is initialized on this thread (above).
        let uia: Option<IUIAutomation2> = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .inspect_err(|e| eprintln!("[selection] UI Automation unavailable, Ctrl+C only: {e}"))
            .ok();
        if let Some(uia) = &uia {
            // Defaults are 2 s / 20 s; a hung app must not freeze us. Timeout = fallback.
            // SAFETY: `uia` is a live interface created on this thread.
            let _ = unsafe { uia.SetConnectionTimeout(300) };
            // SAFETY: as above.
            let _ = unsafe { uia.SetTransactionTimeout(300) };
        }
        Self { uia }
    }

    /// Call right after the hotkey fires. Returns None when nothing is selected.
    pub fn get(&self) -> Option<String> {
        mask_menu_key();
        if let Some(t) = self.uia_selection() {
            eprintln!("[selection] uia");
            return Some(t);
        }
        if NO_CTRL_C_CLASSES.contains(&foreground_class().as_str()) {
            return None;
        }
        let t = from_clipboard();
        if t.is_some() {
            eprintln!("[selection] clipboard");
        }
        t
    }

    fn uia_selection(&self) -> Option<String> {
        let uia = self.uia.as_ref()?;
        // SAFETY: plain COM calls on interfaces that stay alive for the whole block, made on
        // the thread that created them. Errors come back as HRESULTs, mapped to None.
        unsafe {
            let el = uia.GetFocusedElement().ok()?;
            let pattern: IUIAutomationTextPattern = el.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            let ranges = pattern.GetSelection().ok()?;
            let mut parts = Vec::new();
            for i in 0..ranges.Length().ok()? {
                parts.push(ranges.GetElement(i).ok()?.GetText(-1).ok()?.to_string());
            }
            clean(&parts.join("\n"))
        }
    }
}

/// Selections from browsers and Word carry \r\n or lone \r (RichEdit), trailing spaces and
/// runs of blank lines. They show as empty space in the popup and add nothing to the
/// translation. Keeps indentation and paragraph breaks (one blank line). None when only whitespace.
fn clean(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut blank = false;
    for line in s.replace("\r\n", "\n").split(['\r', '\n']).map(str::trim_end) {
        if line.is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        out.push_str(line);
        blank = false;
    }
    (!out.is_empty()).then_some(out)
}

fn foreground_class() -> String {
    let mut buf = [0u16; 128];
    // SAFETY: `buf` is a valid writable buffer; a null window (no foreground) returns 0.
    let n = unsafe { GetClassNameW(GetForegroundWindow(), &mut buf) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0)])
}

fn is_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: no preconditions.
    unsafe { GetAsyncKeyState(i32::from(vk.0)) < 0 }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                // Some apps read scan codes, not virtual keys. Scan codes fit in 16 bits.
                // SAFETY: no preconditions.
                wScan: unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC) } as u16,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) {
    // SAFETY: `inputs` are fully initialized keyboard events; the size argument is INPUT's.
    unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
}

fn mask_menu_key() {
    if [VK_MENU, VK_LWIN, VK_RWIN].into_iter().any(is_down) {
        send(&[key(VK_MASK, false), key(VK_MASK, true)]);
    }
}

fn from_clipboard() -> Option<String> {
    // Ctrl+C while the user still holds Alt arrives as Ctrl+Alt+C, which is not copy.
    let deadline = Instant::now() + Duration::from_millis(500);
    while MODIFIERS.into_iter().any(is_down) && Instant::now() < deadline {
        sleep(Duration::from_millis(5));
    }

    // SAFETY: no preconditions.
    let before = unsafe { GetClipboardSequenceNumber() };
    let saved = clipboard::snapshot();
    send(&[key(VK_CONTROL, false), key(VK_C, false), key(VK_C, true), key(VK_CONTROL, true)]);

    // Sequence number changes on every clipboard write: no fixed sleep.
    let deadline = Instant::now() + Duration::from_millis(300);
    // SAFETY: no preconditions.
    while unsafe { GetClipboardSequenceNumber() } == before {
        if Instant::now() > deadline {
            return None; // nothing copied, so nothing to restore
        }
        sleep(Duration::from_millis(5));
    }
    // Apps may empty the clipboard first and write text a moment later.
    let text = loop {
        match clipboard::read_text() {
            Some(t) => break Some(t),
            None if Instant::now() > deadline => break None,
            None => sleep(Duration::from_millis(5)),
        }
    };
    // Snapshot failed (clipboard busy): keep the copied text rather than wipe the clipboard.
    if let Some(saved) = saved {
        clipboard::restore(saved);
    }
    text.and_then(|t| clean(&t))
}

mod clipboard {
    use std::{thread::sleep, time::Duration};
    use windows::core::w;
    use windows::Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL},
        System::DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
            RegisterClipboardFormatW, SetClipboardData,
        },
        System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        System::Ole::CF_UNICODETEXT,
    };

    // Formats holding GDI handles, not memory: can't be copied as bytes. Windows rebuilds
    // CF_BITMAP from CF_DIB, so images still survive.
    const GDI_FORMATS: [u32; 8] = [2, 3, 9, 14, 0x80, 0x82, 0x83, 0x8E];

    pub type Snapshot = Vec<(u32, Vec<u8>)>;

    /// Proof that this thread has the clipboard open; closes it when dropped.
    struct Open;
    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: an `Open` exists only after OpenClipboard succeeded.
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Other apps (clipboard managers, the app that just copied) may hold it briefly.
    fn open() -> Option<Open> {
        for _ in 0..20 {
            // SAFETY: no preconditions; success is recorded by returning the guard.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open);
            }
            sleep(Duration::from_millis(5));
        }
        None
    }

    // ponytail: copies every format into memory, so a huge Excel range or image copied
    // earlier makes the fallback slower. Lazy restore not possible: Ctrl+C overwrites it.
    pub fn snapshot() -> Option<Snapshot> {
        let open = open()?;
        let mut out = Vec::new();
        let mut f = 0;
        loop {
            // SAFETY: the clipboard is open (`open` guard alive).
            f = unsafe { EnumClipboardFormats(f) };
            if f == 0 {
                break;
            }
            if GDI_FORMATS.contains(&f) {
                continue;
            }
            if let Some(b) = data(&open, f) {
                out.push((f, b));
            }
        }
        Some(out)
    }

    pub fn read_text() -> Option<String> {
        let open = open()?;
        let b = data(&open, u32::from(CF_UNICODETEXT.0))?;
        let (pairs, _) = b.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&units))
    }

    pub fn restore(s: Snapshot) {
        let Some(open) = open() else { return };
        // SAFETY: the clipboard is open (`open` guard alive).
        let _ = unsafe { EmptyClipboard() };
        for (f, b) in &s {
            set(&open, *f, b);
        }
        // Keeps Win+V history from logging the restore as a new entry.
        // SAFETY: `w!` yields a valid null-terminated wide string.
        let exclude = unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) };
        set(&open, exclude, &[0]);
    }

    /// Copy of one clipboard format's bytes. The `Open` guard proves the clipboard is open.
    fn data(_open: &Open, f: u32) -> Option<Vec<u8>> {
        // SAFETY: the clipboard is open. The handle stays owned by the clipboard.
        let h = HGLOBAL(unsafe { GetClipboardData(f) }.ok()?.0);
        // SAFETY: GlobalLock rejects handles that are not global memory (returns null), so
        // private and GDI formats that slipped through are skipped, not misread.
        let p = unsafe { GlobalLock(h) };
        if p.is_null() {
            return None;
        }
        // SAFETY: a locked block is readable for GlobalSize bytes until GlobalUnlock; it is
        // copied out before unlocking.
        let v = unsafe { std::slice::from_raw_parts(p.cast::<u8>(), GlobalSize(h)) }.to_vec();
        // SAFETY: pairs with the successful GlobalLock above.
        let _ = unsafe { GlobalUnlock(h) };
        Some(v)
    }

    /// Puts one format on the clipboard. The `Open` guard proves the clipboard is open.
    fn set(_open: &Open, f: u32, b: &[u8]) {
        // SAFETY: plain allocation; at least 1 byte so empty data still gets a handle.
        let Ok(h) = (unsafe { GlobalAlloc(GMEM_MOVEABLE, b.len().max(1)) }) else { return };
        // SAFETY: `h` was just allocated by us.
        let p = unsafe { GlobalLock(h) };
        if p.is_null() {
            // SAFETY: still ours; nothing else references it.
            let _ = unsafe { GlobalFree(Some(h)) };
            return;
        }
        // SAFETY: the block holds at least b.len() bytes and cannot overlap `b`.
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p.cast::<u8>(), b.len()) };
        // SAFETY: pairs with the GlobalLock above.
        let _ = unsafe { GlobalUnlock(h) };
        // SAFETY: the clipboard is open. On success the system owns the memory; on failure it
        // is still ours to free.
        if unsafe { SetClipboardData(f, Some(HANDLE(h.0))) }.is_err() {
            // SAFETY: the clipboard rejected it, so the memory is still ours.
            let _ = unsafe { GlobalFree(Some(h)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn clean_trims_and_collapses_blank_lines() {
        assert_eq!(clean("\r\n  Hello  \r\n\r\n\r\n \r\n  world\r\n\r\n").unwrap(), "  Hello\n\n  world");
        assert_eq!(clean("a\nb").unwrap(), "a\nb");
        assert_eq!(clean("a\r\nb").unwrap(), "a\nb");
        assert_eq!(clean("a\r\rb  \r").unwrap(), "a\n\nb");
        assert_eq!(clean(" \r\n\t\n"), None);
    }
}
