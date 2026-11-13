// Stamp the keyboard attributes on every host-app text field, current and future.
//
// `autocomplete="off"` goes on every field. It suppresses WebKit's saved-value
// dropdown, which in a WKWebView also paints one frame in the OS appearance
// before it adopts the page's dark scheme. No WebKit or Tauri global toggle
// exists. The dropdown appears on the FIRST focus, so a MutationObserver stamps
// each field as it mounts, rather than a `focusin` listener.
//
// `autocorrect` and `autocapitalize` are `off` on a config field (paths, ids, API
// keys), where an iOS capital silently corrupts the value. A prose field marks
// itself with PROSE_TEXT_ATTRS instead: it keeps sentence capitals, and its
// autocorrect follows the device's Autocorrect switch (`setProseAutocorrect`).
//
// Spell-check is never touched. Apart from a prose field's autocorrect, which the
// switch owns, an attribute a field declares itself wins.
//
// An app frame runs its own stamp (the SDK's `autocorrectStamp.ts`). Both read
// which fields count as text entry from `@lucidos/text-entry`.

import { isTextEntryField } from '@lucidos/text-entry';

// Each attribute is set only when the field hasn't declared it itself, so a
// component that intentionally sets one (e.g. a future autocomplete="username")
// keeps its value. Idempotent — re-stamping a stamped field is a no-op.
function setIfAbsent(el: Element, name: string, value: string): void {
  if (!el.hasAttribute(name)) el.setAttribute(name, value);
}

/** Marks a field as prose — see {@link PROSE_TEXT_ATTRS}. A `data-*` attribute
 *  is never an IDL property, so Preact always routes it through `setAttribute`;
 *  that determinism is the whole point of marking rather than asserting. */
const PROSE_MARKER = 'data-prose';

/** The device's Autocorrect switch, as the stamp last heard it. The store owns
 *  the value and pushes it in, so this module stays free of the store. */
let proseAutocorrect = true;

/** Stamp a single element with the no-autofill attributes if it's a text field. */
export function stampNoAutofill(el: Element, autocorrect = proseAutocorrect): void {
  if (!isTextEntryField(el)) return;
  // Suppressing the saved-value dropdown applies to EVERY field, prose included.
  setIfAbsent(el, 'autocomplete', 'off');
  if (!el.hasAttribute(PROSE_MARKER)) {
    setIfAbsent(el, 'autocorrect', 'off');
    setIfAbsent(el, 'autocapitalize', 'off');
    return;
  }
  // The switch owns a prose field's autocorrect, in both directions, so turning
  // it back on must take the stamp's own `off` away again.
  if (autocorrect) el.removeAttribute('autocorrect');
  else el.setAttribute('autocorrect', 'off');
}

/**
 * Spread onto a **prose field**, one holding sentences the user writes: the chat
 * prompt, a thread title, a trigger intent, an email body.
 *
 * The stamp then leaves its capitalization at the browser default, and turns its
 * autocorrect off only while the device's Autocorrect switch is off. The switch
 * starts on. It exists for an iPhone or iPad whose autocorrect keeps the tap on
 * a button below the text (ADR 0262).
 *
 * It marks rather than asserts. `autocorrect` is a boolean IDL property Preact
 * assigns directly, so `autocorrect="off"` in JSX reflects as ON. A `data-*`
 * attribute always lands through `setAttribute`. `autocomplete` is still stamped
 * on a prose field, which wants the saved-value dropdown suppressed too.
 */
export const PROSE_TEXT_ATTRS = {
  [PROSE_MARKER]: '',
} as const;

/** Stamp `root` (if a text field) and every text field inside it. */
export function sweepNoAutofill(root: ParentNode = document): void {
  if (root instanceof Element) stampNoAutofill(root);
  root.querySelectorAll('input, textarea').forEach((el) => stampNoAutofill(el));
}

/** Take a new value for the device's Autocorrect switch and re-stamp every
 *  mounted field. iOS reads the attribute when a field takes focus, so a field
 *  stamped before the change would otherwise keep the old behavior.
 *
 *  An unchanged value sweeps nothing. Every preferences load lands here, and a
 *  field mounted since the last change was stamped by the observer already. */
export function setProseAutocorrect(enabled: boolean, root: ParentNode = document): void {
  if (enabled === proseAutocorrect) return;
  proseAutocorrect = enabled;
  sweepNoAutofill(root);
}

let installed = false;

/** Install the global no-autofill stamping. Idempotent — safe from module init.
 *  Stamps everything currently in the document, then keeps stamping fields as
 *  they're added (modals, settings panes, the picker) via a MutationObserver.
 *
 *  `autocorrect` is the switch's value at boot, before preferences load, so the
 *  first field to take focus already carries the device's choice. */
export function installNoAutofill(autocorrect: boolean): void {
  if (installed) return;
  installed = true;
  proseAutocorrect = autocorrect;
  sweepNoAutofill(document);
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      record.addedNodes.forEach((node) => {
        if (node instanceof Element) sweepNoAutofill(node);
      });
    }
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
}
