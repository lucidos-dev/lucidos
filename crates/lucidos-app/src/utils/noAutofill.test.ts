import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

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

import { stampNoAutofill, sweepNoAutofill, PROSE_TEXT_ATTRS } from './noAutofill';

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

function prose() {
  const el = textarea();
  for (const [k, v] of Object.entries(PROSE_TEXT_ATTRS)) el.setAttribute(k, v);
  return el;
}

describe('PROSE_TEXT_ATTRS', () => {
  it('leaves a prose field at the browser defaults — no attribute at all', () => {
    const el = prose();
    stampNoAutofill(el);
    // Absent, NOT "on"/"sentences": the browser's own default for a text field
    // is already autocorrect-on + sentence capitalization, so the correct state
    // is the un-stamped one. Asserting a value would route through Preact's
    // property path, which is what inverted AllowlistEditor's "off" into "on".
    expect(el.getAttribute('autocorrect')).toBeNull();
    expect(el.getAttribute('autocapitalize')).toBeNull();
  });

  it('still lets the autofill dropdown be suppressed', () => {
    const el = prose();
    stampNoAutofill(el);
    // The popup + its white→dark flash is what the global stamp exists for, and
    // keeping the keyboard defaults must not opt back into it.
    expect(el.getAttribute('autocomplete')).toBe('off');
    expect(PROSE_TEXT_ATTRS).not.toHaveProperty('autocomplete');
  });

  it('marks via a data-* attribute, which Preact cannot route as a property', () => {
    // The determinism is the point: `data-prose` is never an IDL property, so
    // Preact always emits it with setAttribute. An `autocorrect`/`autocapitalize`
    // key here would reintroduce the property-path seam this replaced.
    expect(Object.keys(PROSE_TEXT_ATTRS)).toEqual(['data-prose']);
  });

  it('does not exempt a non-prose field', () => {
    const el = textarea();
    stampNoAutofill(el);
    expect(el.getAttribute('autocorrect')).toBe('off');
    expect(el.getAttribute('autocapitalize')).toBe('off');
  });
});

// Source-scan guard. The prose fields are the ones a user writes sentences in;
// they were silently stripped of autocorrect + sentence capitalization for a
// month by the global stamp (commit 7c61270ac bundled two attributes into a fix
// that only needed autocomplete). Pin them so a rewrite of one of these fields
// can't drop the opt-out again.
const here: string = dirname(fileURLToPath(import.meta.url));
const read = (p: string): string => readFileSync(resolve(here, p), 'utf-8');

/** Every component source, as [path, comment-stripped contents] — for the JSX
 *  tripwire below. Comments are stripped so prose *about* the banned attribute
 *  (including the one explaining the ban) isn't mistaken for markup. */
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
  '../components/chat/PromptInput.tsx',        // the chat prompt
  '../components/chat/ThreadTitleEditor.tsx',  // thread rename (both layouts)
  '../components/shared/AutoTextarea.tsx',     // app description, new-app description, email body
  '../components/shared/PromptDialog.tsx',     // free-text answer to the LLM
  '../components/email/EmailConfirmModal.tsx', // email subject
  '../components/triggers/TriggerDetails.tsx', // trigger name, group name, intent
];

// Config / code / secret surfaces: an iOS auto-capital here corrupts the value.
const NON_PROSE_FIELDS = [
  '../components/files/FilePreviewInline.tsx',
  '../components/settings/AllowlistEditor.tsx',
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

  // Turning autocorrect off from JSX INVERTS it: `autocorrect` is not in
  // Preact's property-path exclusion list, so it assigns `el.autocorrect =
  // "off"`, and the boolean IDL attribute coerces that non-empty string to
  // true — reflecting `autocorrect="on"`. Worse, the now-present attribute
  // makes setIfAbsent skip the field, so the stamp can never repair it. Only
  // the stamp's setAttribute writes the literal keyword.
  it('no component declares autocorrect="off" in JSX — that turns it ON', () => {
    const offenders = componentSources().filter(([, src]) => /autocorrect=["']off["']/.test(src));
    expect(offenders.map(([p]) => p)).toEqual([]);
  });
});
