//! The page's CSS cursor, mirrored onto the native window.
//!
//! `tao` gives its content view a cursor rect spanning the whole view, carrying
//! the window's own cursor icon (`platform_impl/macos/view.rs`,
//! `reset_cursor_rects`). AppKit re-asserts that rect as the mouse moves, while
//! WebKit sets `NSCursor` from the CSS `cursor` property on the same moves. Two
//! writers, one cursor, so the glyph flickers and the arrow usually wins. A
//! trackpad reports far more movement than a mouse, which is where it is worst.
//!
//! Pointing the window's own icon at what the hovered element asks for makes
//! both writers agree, so there is nothing left to flicker between.
//!
//! A temporary measure: see `docs/temporary-measures.md` § "Native cursor
//! mirroring" for the upstream issues and the removal condition. The page half
//! is `utils/nativeCursor.ts`, which deliberately holds NO table of its own and
//! forwards the computed keyword verbatim, so the two cannot disagree.

use tauri::CursorIcon;

/// Every keyword the CSS `cursor` property accepts, against the icon it names.
///
/// Total by construction: `CursorIcon` has a variant for each of them, so no
/// stylesheet can produce a keyword this table lacks. `auto` and `none` fold
/// onto the arrow, since neither names a glyph we can set (hiding the pointer
/// is `set_cursor_visible`, which nothing asks for).
///
/// `utils/nativeCursor.drift.test.ts` reads these literals and fails when our
/// own CSS declares a cursor missing from here.
const CSS_CURSORS: &[(&str, CursorIcon)] = &[
    ("auto", CursorIcon::Default),
    ("default", CursorIcon::Default),
    ("none", CursorIcon::Default),
    ("context-menu", CursorIcon::ContextMenu),
    ("help", CursorIcon::Help),
    ("pointer", CursorIcon::Hand),
    ("progress", CursorIcon::Progress),
    ("wait", CursorIcon::Wait),
    ("cell", CursorIcon::Cell),
    ("crosshair", CursorIcon::Crosshair),
    ("text", CursorIcon::Text),
    ("vertical-text", CursorIcon::VerticalText),
    ("alias", CursorIcon::Alias),
    ("copy", CursorIcon::Copy),
    ("move", CursorIcon::Move),
    ("no-drop", CursorIcon::NoDrop),
    ("not-allowed", CursorIcon::NotAllowed),
    ("grab", CursorIcon::Grab),
    ("grabbing", CursorIcon::Grabbing),
    ("e-resize", CursorIcon::EResize),
    ("n-resize", CursorIcon::NResize),
    ("ne-resize", CursorIcon::NeResize),
    ("nw-resize", CursorIcon::NwResize),
    ("s-resize", CursorIcon::SResize),
    ("se-resize", CursorIcon::SeResize),
    ("sw-resize", CursorIcon::SwResize),
    ("w-resize", CursorIcon::WResize),
    ("ew-resize", CursorIcon::EwResize),
    ("ns-resize", CursorIcon::NsResize),
    ("nesw-resize", CursorIcon::NeswResize),
    ("nwse-resize", CursorIcon::NwseResize),
    ("col-resize", CursorIcon::ColResize),
    ("row-resize", CursorIcon::RowResize),
    ("all-scroll", CursorIcon::AllScroll),
    ("zoom-in", CursorIcon::ZoomIn),
    ("zoom-out", CursorIcon::ZoomOut),
];

/// The icon a CSS cursor keyword names, or the arrow for anything else.
///
/// The arrow rather than an error, because the caller forwards a value the
/// browser computed rather than one anybody authored. An unrecognised keyword
/// is also what the platform would have shown on its own.
pub fn cursor_icon(css_keyword: &str) -> CursorIcon {
    CSS_CURSORS
        .iter()
        .find(|(keyword, _)| *keyword == css_keyword)
        .map_or(CursorIcon::Default, |(_, icon)| *icon)
}

/// Point the CALLING window's cursor at the CSS keyword the page resolved for
/// the element under the pointer.
///
/// The calling window, never `main`: two windows can be hovered in turn, and
/// the cursor belongs to whichever one the pointer is over. An app command,
/// like `start_window_drag`, so the window-plugin ACL does not apply.
#[tauri::command]
pub fn set_window_cursor(window: tauri::Window, cursor: String) -> Result<(), String> {
    window
        .set_cursor_icon(cursor_icon(&cursor))
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn maps_the_keywords_the_app_declares() {
        assert_eq!(cursor_icon("col-resize"), CursorIcon::ColResize);
        assert_eq!(cursor_icon("pointer"), CursorIcon::Hand);
        assert_eq!(cursor_icon("text"), CursorIcon::Text);
        assert_eq!(cursor_icon("not-allowed"), CursorIcon::NotAllowed);
        assert_eq!(cursor_icon("grabbing"), CursorIcon::Grabbing);
    }

    #[test]
    fn folds_the_glyphless_keywords_onto_the_arrow() {
        assert_eq!(cursor_icon("auto"), CursorIcon::Default);
        assert_eq!(cursor_icon("default"), CursorIcon::Default);
        assert_eq!(cursor_icon("none"), CursorIcon::Default);
    }

    #[test]
    fn an_unknown_keyword_is_the_arrow() {
        assert_eq!(cursor_icon("-webkit-grab"), CursorIcon::Default);
        assert_eq!(cursor_icon(""), CursorIcon::Default);
    }

    /// The whole CSS keyword set, each named once. A short table is easy to
    /// extend by paste, and a duplicate would shadow the entry below it.
    #[test]
    fn the_table_names_every_css_keyword_once() {
        let mut seen = BTreeSet::new();
        for (keyword, _) in CSS_CURSORS {
            assert!(seen.insert(*keyword), "{keyword} is listed twice");
        }
        assert_eq!(seen.len(), 36, "CSS cursor accepts 36 keywords");
    }
}
