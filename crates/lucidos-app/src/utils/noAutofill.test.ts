import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

class AttrEl {
  private attrs = new Map<string, string>();
  hasAttribute(n: string) { return this.attrs.has(n); }
  setAttribute(n: string, v: string) { this.attrs.set(n, v); }
  getAttribute(n: string) { return this.attrs.get(n) ?? null; }
  removeAttribute(n: string) { this.attrs.delete(n); }
}
if (typeof (globalThis as any).Element === 'undefined') {
  (globalThis as any).Element = class Element extends AttrEl {};
}
if (typeof (globalThis as any).HTMLInputElement === 'undefined') {
  (globalThis as any).HTMLInputElement = class HTMLInputElement extends (globalThis as any).Element {
    localName = 'input';
    type = 'text';
  };
}
if (typeof (globalThis as any).HTMLTextAreaElement === 'undefined') {
  (globalThis as any).HTMLTextAreaElement = class HTMLTextAreaElement extends (globalThis as any).Element {
    localName = 'textarea';
  };
}

import { stampNoAutofill, sweepNoAutofill, setProseAutocorrect, PROSE_TEXT_ATTRS } from './noAutofill';

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
    el.setAttribute('autocomplete', 'username');
    stampNoAutofill(el);
    expect(el.getAttribute('autocomplete')).toBe('username');
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

function prose() {
  const el = textarea();
  for (const [k, v] of Object.entries(PROSE_TEXT_ATTRS)) el.setAttribute(k, v);
  return el;
}

describe('PROSE_TEXT_ATTRS', () => {
  it('leaves a prose field at the browser defaults — no attribute at all', () => {
    const el = prose();
    stampNoAutofill(el);
    expect(el.getAttribute('autocorrect')).toBeNull();
    expect(el.getAttribute('autocapitalize')).toBeNull();
  });

  it('still lets the autofill dropdown be suppressed', () => {
    const el = prose();
    stampNoAutofill(el);
    expect(el.getAttribute('autocomplete')).toBe('off');
    expect(PROSE_TEXT_ATTRS).not.toHaveProperty('autocomplete');
  });

  it('marks via a data-* attribute, which Preact cannot route as a property', () => {
    expect(Object.keys(PROSE_TEXT_ATTRS)).toEqual(['data-prose']);
  });

  it('does not exempt a non-prose field', () => {
    const el = textarea();
    stampNoAutofill(el);
    expect(el.getAttribute('autocorrect')).toBe('off');
    expect(el.getAttribute('autocapitalize')).toBe('off');
  });
});

describe('the autocorrect switch', () => {
  it('turns autocorrect off on a prose field when the switch is off', () => {
    const el = prose();
    stampNoAutofill(el, false);
    expect(el.getAttribute('autocorrect')).toBe('off');
  });

  it('keeps sentence capitalization and spell-check on a prose field', () => {
    const el = prose();
    stampNoAutofill(el, false);
    expect(el.getAttribute('autocapitalize')).toBeNull();
    expect(el.getAttribute('spellcheck')).toBeNull();
  });

  it('leaves a non-prose field exactly as it was', () => {
    const el = textarea();
    stampNoAutofill(el, false);
    for (const a of NO_AUTOFILL) expect(el.getAttribute(a), a).toBe('off');
  });

  it('re-stamps every mounted field when the switch flips, both ways', () => {
    const proseField = prose();
    const configField = textarea();
    const root = { querySelectorAll: () => [proseField, configField] } as any;
    sweepNoAutofill(root);
    expect(proseField.getAttribute('autocorrect')).toBeNull();

    setProseAutocorrect(false, root);
    expect(proseField.getAttribute('autocorrect')).toBe('off');
    expect(configField.getAttribute('autocorrect')).toBe('off');

    setProseAutocorrect(true, root);
    expect(proseField.getAttribute('autocorrect')).toBeNull();
    expect(configField.getAttribute('autocorrect')).toBe('off');
  });
});

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (p: string): string => readFileSync(resolve(here, p), 'utf-8');

function componentSources(): Array<[string, string]> {
  const root = resolve(here, '../components');
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e: any) => {
      const full = resolve(dir, e.name);
      return e.isDirectory() ? walk(full) : full.endsWith('.tsx') ? [full] : [];
    });
  return walk(root).map((p: string) => [
    p,
    readFileSync(p, 'utf-8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, ''),
  ]);
}

const PROSE_FIELDS = [
  '../components/chat/PromptInput.tsx',
  '../components/shared/AutoTextarea.tsx',
  '../components/shared/PromptDialog.tsx',
  '../components/email/EmailConfirmModal.tsx',
  '../components/triggers/TriggerDetails.tsx',
  '../components/settings/ResponseStylesSection.tsx',
  '../components/settings/MarketplacesSection.tsx',
  '../components/settings/TitleStyleRow.tsx',
];

const NON_PROSE_FIELDS = [
  '../components/files/FilePreviewInline.tsx',
  '../components/shared/AllowlistRow.tsx',
  '../components/shared/SecretInput.tsx',
];

describe('prose fields opt back into autocorrect', () => {
  it.each(PROSE_FIELDS)('%s spreads PROSE_TEXT_ATTRS', (path) => {
    expect(read(path)).toContain('{...PROSE_TEXT_ATTRS}');
  });

  it('the chat prompt textarea itself carries it (not some sibling field)', () => {
    const source = read('../components/chat/PromptInput.tsx');
    expect(source).toMatch(/data-role="prompt-input"[\s\S]{0,200}?\{\.\.\.PROSE_TEXT_ATTRS\}/);
  });

  it.each(NON_PROSE_FIELDS)('%s stays opted out', (path) => {
    expect(read(path)).not.toContain('PROSE_TEXT_ATTRS');
  });

  it('no component declares autocorrect="off" in JSX — that turns it ON', () => {
    const offenders = componentSources().filter(([, src]) => /autocorrect=["']off["']/.test(src));
    expect(offenders.map(([p]) => p)).toEqual([]);
  });
});
