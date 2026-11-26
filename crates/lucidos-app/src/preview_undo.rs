//! Undo and redo in a URL preview, which the Edit menu otherwise takes from it.
//!
//! wry makes a child webview answer NO to `performKeyEquivalent:` (its
//! workaround for tauri#9426), so Cmd+Z goes straight to the Edit menu. The
//! menu sends a native `undo:`, and the page never sees the keydown. That breaks
//! every page that keeps its own undo stack, Google Docs among them.
//!
//! So a preview gets Safari's order instead, in two halves:
//!
//! 1. A local key monitor hands Cmd+Z and Shift+Cmd+Z to the focused preview as
//!    a plain `keyDown:`, before the menu can match them. The page receives a
//!    trusted keydown.
//! 2. `NATIVE_UNDO_FALLBACK_JS` runs native undo or redo when no page handler
//!    claimed that keydown, so a plain text field still undoes.
//!
//! Only previews are affected: the monitor forwards solely to a webview
//! registered here, and the app's own pages keep the menu path.
//!
//! A temporary measure: `docs/temporary-measures.md` § Undo key monitor for URL
//! previews says when it goes.

use std::collections::BTreeMap;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
pub(crate) const NATIVE_UNDO_FALLBACK_JS: &str = r#"(function(){
    window.addEventListener('keydown',function(e){
        if(!e.isTrusted||!e.metaKey||e.ctrlKey||e.altKey||e.key.toLowerCase()!=='z') return;
        setTimeout(function(){
            if(!e.defaultPrevented) document.execCommand(e.shiftKey?'redo':'undo');
        });
    },true);
})()"#;

#[cfg(any(target_os = "macos", test))]
fn is_undo_or_redo(key: &str, command: bool, control: bool, option: bool) -> bool {
    command && !control && !option && key.eq_ignore_ascii_case("z")
}

static PREVIEW_VIEWS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

pub(crate) fn register(webview: &tauri::Webview) {
    let label = webview.label().to_string();
    let recorded = webview.with_webview(move |platform| {
        #[cfg(target_os = "macos")]
        PREVIEW_VIEWS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(label, platform.inner() as usize);
        #[cfg(not(target_os = "macos"))]
        let _ = (label, platform);
    });
    if let Err(e) = recorded {
        eprintln!("[Tauri] Undo in the preview stays with the menu: {e}");
    }
}

pub(crate) fn unregister(label: &str) {
    PREVIEW_VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(label);
}

#[cfg(target_os = "macos")]
fn is_in_preview(responder: &objc2_app_kit::NSResponder) -> bool {
    let views = PREVIEW_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let mut view: Option<objc2::rc::Retained<objc2_app_kit::NSView>> = responder
        .downcast_ref::<objc2_app_kit::NSView>()
        .map(objc2::rc::Retained::from);
    while let Some(current) = view {
        let address = objc2::rc::Retained::as_ptr(&current) as usize;
        if views.values().any(|&registered| registered == address) {
            return true;
        }
        // SAFETY: the monitor runs on the main thread, which owns the view tree.
        view = unsafe { current.superview() };
    }
    false
}

#[cfg(target_os = "macos")]
pub(crate) fn install_key_monitor() {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags};

    let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for this call.
        let key_event = unsafe { event.as_ref() };
        let flags = key_event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return event.as_ptr();
        }
        let key = key_event
            .charactersIgnoringModifiers()
            .map(|chars| chars.to_string())
            .unwrap_or_default();
        if !is_undo_or_redo(
            &key,
            flags.contains(NSEventModifierFlags::Command),
            flags.contains(NSEventModifierFlags::Control),
            flags.contains(NSEventModifierFlags::Option),
        ) {
            return event.as_ptr();
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return event.as_ptr();
        };
        let focused = NSApplication::sharedApplication(mtm)
            .keyWindow()
            .and_then(|window| window.firstResponder());
        match focused {
            Some(responder) if is_in_preview(&responder) => {
                responder.keyDown(key_event);
                std::ptr::null_mut()
            }
            _ => event.as_ptr(),
        }
    });
    // SAFETY: the handler returns the event it was given, or null to swallow it.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    match monitor {
        Some(token) => std::mem::forget(token),
        None => eprintln!("[Tauri] Undo in the preview stays with the menu: no key monitor"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_z_and_shift_cmd_z_go_to_the_page() {
        assert!(is_undo_or_redo("z", true, false, false));
        assert!(is_undo_or_redo("Z", true, false, false));
    }

    #[test]
    fn other_keys_and_chords_stay_with_the_menu() {
        assert!(!is_undo_or_redo("z", false, false, false));
        assert!(!is_undo_or_redo("z", true, true, false));
        assert!(!is_undo_or_redo("z", true, false, true));
        assert!(!is_undo_or_redo("c", true, false, false));
        assert!(!is_undo_or_redo("", true, false, false));
    }

    #[test]
    fn a_closed_preview_is_forgotten() {
        PREVIEW_VIEWS
            .lock()
            .unwrap()
            .insert("url-preview-test".to_string(), 0xdead);
        unregister("url-preview-test");
        assert!(!PREVIEW_VIEWS
            .lock()
            .unwrap()
            .values()
            .any(|&view| view == 0xdead));
    }
}
