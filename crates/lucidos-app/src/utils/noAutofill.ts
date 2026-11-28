
import { isTextEntryField } from '@lucidos/text-entry';

function setIfAbsent(el: Element, name: string, value: string): void {
  if (!el.hasAttribute(name)) el.setAttribute(name, value);
}

const PROSE_MARKER = 'data-prose';

let proseAutocorrect = true;

export function stampNoAutofill(el: Element, autocorrect = proseAutocorrect): void {
  if (!isTextEntryField(el)) return;
  setIfAbsent(el, 'autocomplete', 'off');
  if (!el.hasAttribute(PROSE_MARKER)) {
    setIfAbsent(el, 'autocorrect', 'off');
    setIfAbsent(el, 'autocapitalize', 'off');
    return;
  }
  if (autocorrect) el.removeAttribute('autocorrect');
  else el.setAttribute('autocorrect', 'off');
}

export const PROSE_TEXT_ATTRS = {
  [PROSE_MARKER]: '',
} as const;

export function sweepNoAutofill(root: ParentNode = document): void {
  if (root instanceof Element) stampNoAutofill(root);
  root.querySelectorAll('input, textarea').forEach((el) => stampNoAutofill(el));
}

export function setProseAutocorrect(enabled: boolean, root: ParentNode = document): void {
  if (enabled === proseAutocorrect) return;
  proseAutocorrect = enabled;
  sweepNoAutofill(root);
}

let installed = false;

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
