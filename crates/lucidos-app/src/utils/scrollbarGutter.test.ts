import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { measureScrollbarGutter, publishScrollbarGutter } from './scrollbarGutter';

interface FakeTranscript {
  reserved: number;
  offsetWidth?: number;
  overflowY?: string;
  borderWidth?: string;
}

function fakeDoc(probeReserved: number, transcripts: FakeTranscript[] = []) {
  const attached: unknown[] = [];
  const cssTexts: string[] = [];
  const vars: Record<string, string> = {};
  const styles = new Map<unknown, Record<string, string>>();
  const els = transcripts.map(t => {
    const offsetWidth = t.offsetWidth ?? 800;
    const el = { offsetWidth, clientWidth: offsetWidth - t.reserved };
    styles.set(el, {
      overflowY: t.overflowY ?? 'scroll',
      borderLeftWidth: t.borderWidth ?? '0px',
      borderRightWidth: t.borderWidth ?? '0px',
    });
    return el;
  });
  const doc = {
    createElement: () => ({
      style: {
        get cssText() { return cssTexts[cssTexts.length - 1] ?? ''; },
        set cssText(v: string) { cssTexts.push(v); },
      },
      remove() { attached.pop(); },
      offsetWidth: 100,
      clientWidth: 100 - probeReserved,
    }),
    querySelectorAll: (sel: string) => (sel === '.thread-content' ? els : []),
    defaultView: { getComputedStyle: (el: unknown) => styles.get(el) },
    body: { appendChild: (el: unknown) => { attached.push(el); } },
    documentElement: {
      style: { setProperty: (k: string, v: string) => { vars[k] = v; } },
    },
  };
  return { doc: doc as unknown as Document, attached, cssTexts, vars };
}

function threadContentRule(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  const css = readFileSync(resolve(here, '../styles/chat/input-messages.css'), 'utf-8');
  const start = css.indexOf('\n.thread-content {');
  expect(start, '.thread-content rule not found in chat/input-messages.css').toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

describe('measureScrollbarGutter', () => {
  it('reports what the LIVE transcript reserved, not what the probe guessed', () => {
    const { doc } = fakeDoc(9, [{ reserved: 0 }]);
    expect(measureScrollbarGutter(doc)).toBe(0);
  });

  it('reports the live transcript on a classic-scrollbar platform too', () => {
    const { doc } = fakeDoc(0, [{ reserved: 15 }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('does not measure the probe at all once a transcript is mounted', () => {
    const { doc, cssTexts, attached } = fakeDoc(9, [{ reserved: 0 }]);
    measureScrollbarGutter(doc);
    expect(cssTexts, 'the probe was laid out even though a transcript exists').toHaveLength(0);
    expect(attached).toHaveLength(0);
  });

  it('discounts the transcript element own borders', () => {
    const { doc } = fakeDoc(0, [{ reserved: 17, borderWidth: '1px' }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('skips a 0-width copy from an inactive layout', () => {
    const { doc } = fakeDoc(0, [{ reserved: 0, offsetWidth: 0 }, { reserved: 15 }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('skips the compose-empty welcome, which is not a scroll container', () => {
    const { doc } = fakeDoc(15, [{ reserved: 0, overflowY: 'visible' }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('falls back to the probe before any transcript is mounted', () => {
    const { doc } = fakeDoc(8);
    expect(measureScrollbarGutter(doc)).toBe(8);
  });

  it('reports 0 where scrollbars are overlay (iOS, default macOS)', () => {
    const { doc } = fakeDoc(0);
    expect(measureScrollbarGutter(doc)).toBe(0);
  });

  it('probes with the transcript scroll container declarations, verbatim', () => {
    const { doc, cssTexts } = fakeDoc(8);
    measureScrollbarGutter(doc);
    expect(cssTexts).toHaveLength(1);
    const probed = cssTexts[0].replace(/\s/g, '');
    const rule = threadContentRule().replace(/\s/g, '');
    for (const decl of ['overflow-x:hidden', 'overflow-y:scroll', 'scrollbar-gutter:stable']) {
      expect(rule, `.thread-content no longer declares ${decl}`).toContain(decl);
      expect(probed, `the probe no longer declares ${decl}`).toContain(decl);
    }
  });

  it('leaves no probe behind in the document', () => {
    const { doc, attached } = fakeDoc(8);
    measureScrollbarGutter(doc);
    expect(attached).toHaveLength(0);
  });

  it('returns 0 without throwing in a layout-less environment', () => {
    const doc = {
      createElement: () => ({ tagName: 'DIV' }),
      querySelector: () => null,
    } as unknown as Document;
    expect(measureScrollbarGutter(doc)).toBe(0);
  });
});

describe('publishScrollbarGutter', () => {
  it('publishes the measured width in px', () => {
    const { doc, vars } = fakeDoc(10);
    expect(publishScrollbarGutter(doc)).toBe(10);
    expect(vars['--scrollbar-gutter-width']).toBe('10px');
  });

  it('publishes 0px on an overlay-scrollbar platform, matching the CSS default', () => {
    const { doc, vars } = fakeDoc(0);
    expect(publishScrollbarGutter(doc)).toBe(0);
    expect(vars['--scrollbar-gutter-width']).toBe('0px');
  });

  it('republishes the live transcript width over the boot estimate', () => {
    const { doc, vars } = fakeDoc(9, [{ reserved: 0 }]);
    expect(publishScrollbarGutter(doc)).toBe(0);
    expect(vars['--scrollbar-gutter-width']).toBe('0px');
  });
});
