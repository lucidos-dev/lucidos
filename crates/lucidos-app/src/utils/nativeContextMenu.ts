// Decide where the platform's own context menu shows, in the desktop app.
//
// ADR 0285 has the rule. An object with actions opens its own ⋯ menu, and its
// handler claims the event first. Content keeps the native menu: Look Up,
// Translate, Copy and Services are OS features we cannot rebuild. Chrome gets
// nothing, because WebKit's menu there offers Reload, Inspect Element and
// AutoFill, and Reload discards the running app.
//
// Scoped to Tauri. In a browser tab the page menu belongs to the browser.
// The decision itself is shared with widget frames (`@lucidos/context-menu`).

import { isTauri } from './platform';
import { opensContextMenu, selectionCovers, shouldSuppressContextMenu, textAt } from '@lucidos/context-menu';

let installed = false;

/** Install the listeners. Idempotent, and a no-op off Tauri. */
export function installNativeContextMenuPolicy(): void {
  if (installed) return;
  installed = true;
  if (!isTauri()) return;

  let menuPress = false;
  let selectionAtPress = false;
  // Capture phase, so a row that stops propagation cannot hide its press.
  // Only a right-click or ctrl-click measures, so an ordinary click never
  // walks the rects of a long selection.
  document.addEventListener('pointerdown', (e) => {
    menuPress = opensContextMenu(e);
    selectionAtPress = menuPress
      && selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
  }, { capture: true });

  // Bubble phase, so every element's own handler has run and had its chance to
  // claim the event. Never stops propagation.
  document.addEventListener('contextmenu', (e) => {
    const point = { x: e.clientX, y: e.clientY };
    // A menu raised by the keyboard or VoiceOver had no press, and its point
    // means nothing. In a text region it keeps the native menu.
    const fromPointer = menuPress;
    const heldSelection = selectionAtPress;
    menuPress = selectionAtPress = false;
    const textUnderPointer = () => !fromPointer || textAt(e.target, point);
    if (!shouldSuppressContextMenu(e, heldSelection, textUnderPointer)) return;
    e.preventDefault();
    // Undo the word WebKit selected on the way in, so no highlight is left on
    // the chrome. The press started with no selection under the pointer, so
    // one there now is WebKit's. A selection elsewhere is the user's: keep it.
    const selection = window.getSelection();
    if (selectionCovers(selection, point)) selection?.removeAllRanges();
  });
}
