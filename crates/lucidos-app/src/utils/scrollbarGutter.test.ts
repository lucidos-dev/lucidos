import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { measureScrollbarGutter, publishScrollbarGutter } from './scrollbarGutter';

interface FakeTranscript {
  /** Width the element takes out of its own content box (scrollbar + borders). */
  reserved: number;
  /** 0 for an inactive layout's copy, which must never be the one measured. */
  offsetWidth?: number;
  /** `visible` is the compose-empty welcome, which reserves nothing. */
  overflowY?: string;
  borderWidth?: string;
}

// The test env is `node` (no jsdom), so stub just enough document: some number of
// live `.thread-content` elements, plus a probe element whose clientWidth is
// narrower than its offsetWidth by whatever the simulated platform reserves.
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

/** The `.thread-content` rule body, straight out of the stylesheet. */
function threadContentRule(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  const css = readFileSync(resolve(here, '../styles/chat/input-messages.css'), 'utf-8');
  const start = css.indexOf('\n.thread-content {');
  expect(start, '.thread-content rule not found in chat/input-messages.css').toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', start));
}

describe('measureScrollbarGutter', () => {
  it('reports what the LIVE transcript reserved, not what the probe guessed', () => {
    // The iOS split this exists for: the detached probe picks up the
    // ::-webkit-scrollbar width (0.5rem = 9px at the mobile root) while the real
    // scroll container reserves nothing, so a probe-derived value pushed the
    // composer's right edge 9px inside the cards it docks under.
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
    // offsetWidth - clientWidth is gutter + borders; only the gutter is the
    // composer's to subtract.
    const { doc } = fakeDoc(0, [{ reserved: 17, borderWidth: '1px' }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('skips a 0-width copy from an inactive layout', () => {
    const { doc } = fakeDoc(0, [{ reserved: 0, offsetWidth: 0 }, { reserved: 15 }]);
    expect(measureScrollbarGutter(doc)).toBe(15);
  });

  it('skips the compose-empty welcome, which is not a scroll container', () => {
    // It reuses `.thread-content` with `overflow: visible` and reserves nothing.
    // Letting it answer would drop the compensation from the composer while it is
    // still the same element about to dock under a real transcript, so the box
    // would slide sideways on the way into a thread. Falls back to the probe.
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
    // The fallback probe is only trustworthy as a CLONE: engines disagree about
    // whether `overflow-y: scroll` or `scrollbar-gutter: stable` is what reserves
    // the space (Chromium honours the stable gutter without overflow, WebKit needs
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

  it('republishes the live transcript width over the boot estimate', () => {
    const { doc, vars } = fakeDoc(9, [{ reserved: 0 }]);
    expect(publishScrollbarGutter(doc)).toBe(0);
    expect(vars['--scrollbar-gutter-width']).toBe('0px');
  });
});
