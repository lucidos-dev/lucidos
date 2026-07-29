// Globally suppress the browser's saved-value autofill dropdown (and WebKit's
// autocorrect/autocapitalize text munging) on every host-app text field.
//
// WHY: a focused <input>/<textarea> the engine treats as a form field gets
// WebKit's saved-value popup (the "Te ×" chip — × deletes a remembered entry).
// In a Tauri WKWebView (and Safari) that popup also paints one frame in the OS
// appearance before it adopts the page's `color-scheme: dark`, so it flashes
// white→dark. Suppressing the popup removes both the popup AND its flash.
//
// HOW: the portable fix is the per-field attributes (no WKWebView/Tauri global
// toggle exists). Rather than annotate ~40 components by hand (which new code
// would keep forgetting), stamp them centrally: an initial sweep covers what's
// already mounted, and a MutationObserver covers everything mounted later
// (modals, settings, search, the picker). The observer — not a cheaper
// `focusin` listener — is required because the popup appears on the FIRST focus,
// so the attribute has to be present before the field is ever focused.
//
// Kept deliberately narrow: spellcheck is left alone (red-squiggle typo help
// still works in chat), and locally-set attributes win — we only stamp a field
// that hasn't already declared the attribute itself.
//
// A field the user writes SENTENCES in opts out of the autocorrect/autocapitalize
// half by marking itself instead: see PROSE_TEXT_ATTRS below. It's a separate
// mechanism from "locally-set attributes win" on purpose — declaring
// autocorrect="off" from JSX inverts to ON, so the marker is the only safe seam.

// input types that are NOT text entry — autofill/autocorrect don't apply, so
// skip them. Allow-by-exclusion so future text-ish types (date, month, …) are
// covered without a list edit.
const NON_TEXT_INPUT_TYPES = new Set([
  'button', 'submit', 'reset', 'image', 'file',
  'checkbox', 'radio', 'range', 'color', 'hidden',
]);

function isAutofillTarget(el: Element): el is HTMLInputElement | HTMLTextAreaElement {
  if (el instanceof HTMLTextAreaElement) return true;
  if (el instanceof HTMLInputElement) return !NON_TEXT_INPUT_TYPES.has(el.type);
  return false;
}

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

/** Stamp a single element with the no-autofill attributes if it's a text field. */
export function stampNoAutofill(el: Element): void {
  if (!isAutofillTarget(el)) return;
  // Suppressing the saved-value dropdown applies to EVERY field, prose included.
  setIfAbsent(el, 'autocomplete', 'off');
  // A prose field is left at the browser's defaults — autocorrect on, sentence
  // capitalization — by simply not being stamped. See PROSE_TEXT_ATTRS.
  if (el.hasAttribute(PROSE_MARKER)) return;
  setIfAbsent(el, 'autocorrect', 'off');
  setIfAbsent(el, 'autocapitalize', 'off');
}

/**
 * Spread onto a **prose field** — one holding natural language the user writes
 * (the chat prompt, a thread title, a trigger intent, an app description, an
 * email body) — so the stamp leaves its keyboard behaviour alone.
 *
 * The stamp turns autocorrect + autocapitalize off, which is right for the ~100
 * config fields this app is mostly made of (paths, ids, env var names, model
 * ids, API keys, allowlist entries) where an iOS auto-capital silently corrupts
 * the value — and wrong for the handful of fields the user writes sentences in.
 *
 * It marks rather than asserts, deliberately. The browser's own defaults for a
 * text field ARE autocorrect-on and sentence capitalization, so a prose field
 * needs no attributes at all — it needs the stamp to keep its hands off. Two
 * reasons that beats spreading `autocorrect="on" autocapitalize="sentences"`:
 *
 *  1. It restores the exact pre-stamp markup, which is the behaviour every
 *     browser is tuned for, instead of asserting a value and hoping the engine
 *     treats an explicit keyword identically to its default.
 *  2. It never touches Preact's property path. `autocorrect` is NOT in Preact's
 *     property-path exclusion list, so `autocorrect={x}` becomes
 *     `el.autocorrect = x` — and the IDL attribute is a *boolean*, so the
 *     non-empty string `"off"` coerces to `true` and reflects back as
 *     `autocorrect="on"`. That inversion is a real bug this codebase shipped
 *     (AllowlistEditor, 9 Jun–29 Jul). A `data-*` attribute is never an IDL
 *     property, so this marker always lands via `setAttribute`.
 *
 * `autocomplete` is deliberately still stamped on a prose field: it wants the
 * saved-value dropdown (and its white→dark flash) suppressed like everything
 * else. Turning autocorrect OFF remains the stamp's job and must never be done
 * from JSX — see the inversion above.
 */
export const PROSE_TEXT_ATTRS = {
  [PROSE_MARKER]: '',
} as const;

/** Stamp `root` (if a text field) and every text field inside it. */
export function sweepNoAutofill(root: ParentNode = document): void {
  if (root instanceof Element) stampNoAutofill(root);
  root.querySelectorAll('input, textarea').forEach(stampNoAutofill);
}

let installed = false;

/** Install the global no-autofill stamping. Idempotent — safe from module init.
 *  Stamps everything currently in the document, then keeps stamping fields as
 *  they're added (modals, settings panes, the picker) via a MutationObserver. */
export function installNoAutofill(): void {
  if (installed) return;
  installed = true;
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
