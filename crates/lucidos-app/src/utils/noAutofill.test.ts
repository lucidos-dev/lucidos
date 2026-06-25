import { describe, it, expect } from 'vitest';

// The test env is `node` (no jsdom) — stub the DOM classes stampNoAutofill keys
// off, matching the dom.test.ts pattern. Each stub carries a real attribute map
// so hasAttribute/setAttribute behave like the DOM.
class AttrEl {
  private attrs = new Map<string, string>();
  hasAttribute(n: string) { return this.attrs.has(n); }
  setAttribute(n: string, v: string) { this.attrs.set(n, v); }
  getAttribute(n: string) { return this.attrs.get(n) ?? null; }
}
if (typeof (globalThis as any).Element === 'undefined') {
  (globalThis as any).Element = class Element extends AttrEl {};
}
if (typeof (globalThis as any).HTMLInputElement === 'undefined') {
  (globalThis as any).HTMLInputElement = class HTMLInputElement extends (globalThis as any).Element {
    type = 'text';
  };
}
if (typeof (globalThis as any).HTMLTextAreaElement === 'undefined') {
  (globalThis as any).HTMLTextAreaElement = class HTMLTextAreaElement extends (globalThis as any).Element {};
}

import { stampNoAutofill, sweepNoAutofill } from './noAutofill';

function input(type = 'text') {
  const el = new (globalThis as any).HTMLInputElement();
  el.type = type;
  return el as any;
}
function textarea() {
  return new (globalThis as any).HTMLTextAreaElement() as any;
}

const NO_AUTOFILL = ['autocomplete', 'autocorrect', 'autocapitalize'] as const;

describe('stampNoAutofill', () => {
  it('stamps all three off-attributes on a text input', () => {
    const el = input('text');
    stampNoAutofill(el);
    for (const a of NO_AUTOFILL) expect(el.getAttribute(a), a).toBe('off');
  });

  it('stamps a textarea', () => {
    const el = textarea();
    stampNoAutofill(el);
    for (const a of NO_AUTOFILL) expect(el.getAttribute(a), a).toBe('off');
  });

  it('stamps text-ish input types (search, email, url, tel, number, date, future types)', () => {
    for (const t of ['search', 'email', 'url', 'tel', 'number', 'date', 'datetime-local', 'week']) {
      const el = input(t);
      stampNoAutofill(el);
      expect(el.getAttribute('autocomplete'), `type=${t}`).toBe('off');
    }
  });

  it('leaves spellcheck untouched (red-squiggle typo help stays on)', () => {
    const el = input('text');
    stampNoAutofill(el);
    expect(el.getAttribute('spellcheck')).toBeNull();
  });

  it('respects a locally-declared attribute and only fills the absent ones', () => {
    const el = input('text');
    el.setAttribute('autocomplete', 'username'); // component opted in explicitly
    stampNoAutofill(el);
    expect(el.getAttribute('autocomplete')).toBe('username'); // not clobbered
    expect(el.getAttribute('autocorrect')).toBe('off');
    expect(el.getAttribute('autocapitalize')).toBe('off');
  });

  it('skips non-text input types (no keyboard / not a form value)', () => {
    for (const t of ['button', 'submit', 'reset', 'image', 'file',
                     'checkbox', 'radio', 'range', 'color', 'hidden']) {
      const el = input(t);
      stampNoAutofill(el);
      for (const a of NO_AUTOFILL) expect(el.getAttribute(a), `type=${t} attr=${a}`).toBeNull();
    }
  });

  it('ignores non-field elements without throwing', () => {
    const div = new (globalThis as any).Element();
    expect(() => stampNoAutofill(div)).not.toThrow();
    for (const a of NO_AUTOFILL) expect(div.getAttribute(a)).toBeNull();
  });
});

describe('sweepNoAutofill', () => {
  it('stamps every field returned by querySelectorAll', () => {
    const fields = [input('text'), textarea(), input('search')];
    const root = { querySelectorAll: () => fields } as any;
    sweepNoAutofill(root);
    for (const el of fields) {
      for (const a of NO_AUTOFILL) expect(el.getAttribute(a)).toBe('off');
    }
  });
});
