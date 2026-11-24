import {
  AUTOCORRECT_STORAGE_KEY, isTextEntryField, resolveAutocorrect,
} from './textEntry';
import { wsLocalGet } from './_storage';
import { servedPrefs } from './boot/appearanceBoot';

let autocorrect = true;
let stamped = new WeakSet<Element>();
let observer: MutationObserver | null = null;

function stampOnFocus(event: FocusEvent): void {
  if (event.target instanceof Element) stamp(event.target);
}

function stamp(el: Element): void {
  if (!isTextEntryField(el)) return;
  if (autocorrect) {
    if (stamped.delete(el) && el.getAttribute('autocorrect') === 'off') {
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

export function initialAutocorrect(): boolean {
  const seeded = servedPrefs()?.['autocorrect'];
  const raw = typeof seeded === 'string' ? seeded : wsLocalGet(AUTOCORRECT_STORAGE_KEY);
  return resolveAutocorrect(raw);
}

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

export function setAppAutocorrect(enabled: boolean): void {
  if (enabled === autocorrect) return;
  autocorrect = enabled;
  if (observer) sweep(document);
}

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
