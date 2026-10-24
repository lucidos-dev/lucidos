/**
 * The device's Autocorrect switch, applied to an app frame's own text fields.
 *
 * The host stamps its fields in its own document (`utils/noAutofill.ts`), and
 * that stamp cannot reach this one. So the SDK stamps here, by the same rule:
 * while the switch is off, every text field gets `autocorrect="off"`. iOS
 * autocorrect can otherwise keep a tap on a button below the text (ADR 0262).
 *
 * Three rules hold:
 *
 * - **Before first focus.** iOS reads the attribute when a field takes focus, so
 *   a MutationObserver stamps each field as it mounts, and focus stamps too.
 * - **Only `autocorrect`.** Capitals and spell-check are the app's.
 * - **The author wins.** A field that declares `autocorrect` itself keeps it.
 *   The stamp remembers the fields it wrote, and turning the switch on removes
 *   only those.
 */
import {
  AUTOCORRECT_STORAGE_KEY, isTextEntryField, resolveAutocorrect,
} from './textEntry';
import { wsLocalGet } from './_storage';
import { servedPrefs } from './boot/appearanceBoot';

/** The switch as this frame last heard it. */
let autocorrect = true;
/** Fields whose `autocorrect="off"` this module wrote, and may take back. */
let stamped = new WeakSet<Element>();
let observer: MutationObserver | null = null;

/** A field focused in the task that mounted it, `autofocus` for one, takes focus
 *  before the observer delivers. So focus stamps too. `focus` rather than
 *  `focusin`, which fires after it, and in the capture phase, so the stamp lands
 *  before any handler on the field. */
function stampOnFocus(event: FocusEvent): void {
  if (event.target instanceof Element) stamp(event.target);
}

function stamp(el: Element): void {
  if (!isTextEntryField(el)) return;
  if (autocorrect) {
    if (stamped.delete(el) && el.getAttribute('autocorrect') === 'off') {
      // Still the SDK's value. One the app changed since is the app's now.
      el.removeAttribute('autocorrect');
    }
    return;
  }
  if (el.hasAttribute('autocorrect')) return;
  el.setAttribute('autocorrect', 'off');
  stamped.add(el);
}

function sweep(root: ParentNode): void {
  if (root instanceof Element) stamp(root);
  root.querySelectorAll('input, textarea').forEach(stamp);
}

/**
 * What the switch reads before this frame's preferences arrive.
 *
 * The engine's first-paint seed comes first. An app that loads
 * `/api/v1/sdk-prefs.js` carries it, and it is the only synchronous source an
 * isolated frame has. The shell's mirror comes next, for a frame that can read
 * the shell's storage. The default, which is on, comes last.
 */
export function initialAutocorrect(): boolean {
  const seeded = servedPrefs()?.['autocorrect'];
  const raw = typeof seeded === 'string' ? seeded : wsLocalGet(AUTOCORRECT_STORAGE_KEY);
  return resolveAutocorrect(raw);
}

/**
 * Stamp every text field in the document, then keep stamping fields as they
 * mount. Idempotent. `enabled` is the switch at SDK load; the device's
 * preferences correct it through {@link setAppAutocorrect}.
 */
export function installAutocorrectStamp(enabled: boolean = initialAutocorrect()): void {
  if (observer) return;
  autocorrect = enabled;
  sweep(document);
  observer = new MutationObserver((records) => {
    for (const record of records) {
      record.addedNodes.forEach((node) => {
        if (node instanceof Element) sweep(node);
      });
    }
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });
  document.addEventListener('focus', stampOnFocus, true);
}

/**
 * Take the device's resolved switch and re-stamp every mounted field. iOS
 * reads the attribute at focus, so a field stamped before the change would
 * otherwise keep the old behavior. An unchanged value sweeps nothing.
 *
 * Before {@link installAutocorrectStamp} it only records the value. The host
 * shell imports the SDK too, and its fields belong to its own stamp.
 */
export function setAppAutocorrect(enabled: boolean): void {
  if (enabled === autocorrect) return;
  autocorrect = enabled;
  if (observer) sweep(document);
}

/** Resolve a fetched preferences map and apply it. */
export function applyAutocorrectPreference(prefs: Record<string, string>): void {
  setAppAutocorrect(resolveAutocorrect(prefs['autocorrect']));
}

export function _resetAutocorrectStampForTesting(): void {
  observer?.disconnect();
  observer = null;
  document.removeEventListener('focus', stampOnFocus, true);
  autocorrect = true;
  stamped = new WeakSet();
}
