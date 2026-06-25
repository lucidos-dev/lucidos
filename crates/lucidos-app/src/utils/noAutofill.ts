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

/** Stamp a single element with the no-autofill attributes if it's a text field. */
export function stampNoAutofill(el: Element): void {
  if (!isAutofillTarget(el)) return;
  setIfAbsent(el, 'autocomplete', 'off');
  setIfAbsent(el, 'autocorrect', 'off');
  setIfAbsent(el, 'autocapitalize', 'off');
}

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
