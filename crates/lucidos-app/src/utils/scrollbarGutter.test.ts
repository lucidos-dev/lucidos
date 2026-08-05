import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { measureScrollbarGutter, publishScrollbarGutter } from './scrollbarGutter';

// The test env is `node` (no jsdom), so stub just enough document for the probe:
// an element whose clientWidth is narrower than its offsetWidth by whatever the
// simulated platform reserves for a scrollbar.
function fakeDoc(reserved: number) {
  const attached: unknown[] = [];
  const cssTexts: string[] = [];
  const vars: Record<string, string> = {};
  const doc = {
    createElement: () => ({
      style: {
        get cssText() { return cssTexts[cssTexts.length - 1] ?? ''; },
        set cssText(v: string) { cssTexts.push(v); },
      },
      remove() { attached.pop(); },
      offsetWidth: 100,
      clientWidth: 100 - reserved,
    }),
    body: { appendChild: (el: unknown) => { attached.push(el); } },
    documentElement: {
      style: { setProperty: (k: string, v: string) => { vars[k] = v; } },
    },
  };
  return { doc: doc as unknown as Document, attached, cssTexts, vars };
}

/** The `.thread-content` rule body, straight out of the stylesheet. */
function threadContentRule(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  const css = readFileSync(resolve(here, '../styles/chat/input-messages.css'), 'utf-8');
  const start = css.indexOf('\n.thread-content {');
  expect(start, '.thread-content rule not found in chat/input-messages.css').toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

describe('measureScrollbarGutter', () => {
  it('reports the reserved width where scrollbars are classic', () => {
    const { doc } = fakeDoc(8);
    expect(measureScrollbarGutter(doc)).toBe(8);
  });

  it('reports 0 where scrollbars are overlay (iOS, default macOS)', () => {
    const { doc } = fakeDoc(0);
    expect(measureScrollbarGutter(doc)).toBe(0);
  });

  it('probes with the transcript scroll container declarations, verbatim', () => {
    // The probe is only trustworthy as a CLONE: engines disagree about whether
    // `overflow-y: scroll` or `scrollbar-gutter: stable` is what reserves the
    // space (Chromium honours the stable gutter without overflow, WebKit needs
    // the scrollbar drawn, headless Chromium zeroes the scrollbar but not the
    // gutter), so a probe missing either declaration answers a different
    // question than the one .thread-content asks.
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
});
