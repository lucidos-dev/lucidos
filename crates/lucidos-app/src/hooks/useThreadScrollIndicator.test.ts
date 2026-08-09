import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { INDICATOR_HIDE_DELAY_MS } from './useThreadScrollIndicator';
// `atRules` is what lets these tests assert WHERE a rule applies, which is the
// whole point here: the suppression and the replacement have to share one gate.
import { cssRules } from '../styles/__tests__/css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, '..');
const read = (rel: string) => readFileSync(resolve(src, rel), 'utf8');

/** Comments out of the TypeScript source, so the scans below read CODE and not
 *  the prose explaining it. The hook documents the very patterns it bans, so an
 *  unstripped scan matches its own rationale. (The CSS needs no equivalent:
 *  postcss parses comments into their own nodes.) */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/^\s*\/\/.*$/gm, ' ');
}

// Source scans rather than behavioral tests, for the same reason
// useHideOnScroll.test.ts uses them: this suite has no DOM, and the regressions
// below are about WHICH property and WHICH element get written, not about a
// value the pure geometry helper already covers
// (components/chat/__tests__/scroll-indicator-geometry.test.ts).
const hookCode = stripComments(read('hooks/useThreadScrollIndicator.ts'));

describe('the per-scroll path stays composited', () => {
  // A layout write in the scroll path dirties layout, so the very next scroll
  // event's scrollTop read forces a synchronous style+layout flush of the
  // largest DOM in the app. That is the jank useHideOnScroll.ts already paid for
  // once (moving --mobile-header-offset off `top` and onto `transform`); this
  // indicator must not reintroduce it by animating `height` or `top`.
  it('writes only composited or paint-only properties as inline styles', () => {
    // `transform` and `opacity` are composited. `borderRadius` is paint-only: it
    // changes the painted shape of a box without moving or resizing anything, so
    // it cannot dirty layout either. It is written per frame to undo `scaleY`'s
    // distortion of the thumb's caps (see counterScaledRadiusPx). Anything not on
    // this list needs the same argument made for it before being added.
    const written = new Set(
      [...hookCode.matchAll(/\.style\.([A-Za-z][A-Za-z0-9]*)\s*=/g)].map(m => m[1]),
    );
    expect([...written].sort()).toEqual(['borderRadius', 'opacity', 'transform']);
  });

  it('never writes a layout property on the thumb', () => {
    for (const prop of ['height', 'top', 'bottom', 'width', 'left', 'right', 'margin', 'padding']) {
      expect(hookCode).not.toMatch(new RegExp(`\\.style\\.${prop}\\s*=`));
    }
  });

  it('never writes a custom property, on the document root or anywhere else', () => {
    // Custom properties inherit, so a scroll-frequency write on documentElement
    // invalidates style for every node in the document.
    expect(hookCode).not.toMatch(/setProperty\s*\(/);
    expect(hookCode).not.toMatch(/documentElement/);
  });
});

describe('the fade is a post-motion linger, not a scroll budget', () => {
  it('is long enough to bridge the gaps between momentum frames', () => {
    // Restarted by every real movement, so it only ever runs to completion after
    // the scroller has actually stopped. It must still comfortably exceed the
    // spacing between late momentum frames, which stretches as a fling decays.
    expect(INDICATOR_HIDE_DELAY_MS).toBeGreaterThan(500);
  });

  it('does not outstay the scroll it belongs to', () => {
    expect(INDICATOR_HIDE_DELAY_MS).toBeLessThan(3000);
  });

  it('does not depend on the touch window, which no longer bounds visibility', () => {
    // The original bug: keep-alive was gated on isUserScrolling(), so the
    // indicator could only survive USER_SCROLL_WINDOW_MS + the delay past the
    // finger lifting, and a longer fling outlived it. Keep-alive now tracks
    // motion, so the two constants are independent. Pinned so a future edit
    // cannot quietly re-couple them.
    expect(hookCode).not.toMatch(/USER_SCROLL_WINDOW_MS/);
  });
});

describe('the thumb box is measured with the right API for each axis', () => {
  // The two are not interchangeable, and swapping either one fails silently.
  it('reads the bar width at sub-pixel precision, never through offsetWidth', () => {
    // offsetWidth rounds to a whole pixel. A rem-authored width rarely lands on
    // one, and rounding UP makes the two corner radii on an edge sum to more
    // than the width, at which point CSS scales EVERY radius down by the
    // overflow ratio, vertical ones included. That silently defeats the
    // counter-scale and the caps stop being semicircles.
    expect(hookCode).not.toMatch(/offsetWidth/);
    expect(hookCode).toMatch(/getBoundingClientRect\(\)\.width/);
  });

  it('reads the scale base through offsetHeight, which the transform does not touch', () => {
    // getBoundingClientRect reports the TRANSFORMED box, so using it for the
    // base would feed the already-scaled height back in and compound the scale
    // frame over frame.
    expect(hookCode).toMatch(/offsetHeight/);
    expect(hookCode).not.toMatch(/getBoundingClientRect\(\)\.height/);
  });
});

describe('the visibility rule lives in one testable place', () => {
  it('routes every scroll event through nextIndicatorVisibility', () => {
    // Re-deriving the rule inline is how it went wrong the first time: the
    // summon signal (touch) got reused as the keep-alive signal (motion). The
    // pure function is where that distinction is stated and covered
    // (components/chat/__tests__/scroll-indicator-geometry.test.ts).
    expect(hookCode).toMatch(/nextIndicatorVisibility\(/);
    // No inline `if (isUserScrolling())` branch deciding visibility.
    expect(hookCode).not.toMatch(/if\s*\(\s*isUserScrolling\(\)\s*\)/);
  });
});

describe('the native indicator is only suppressed where a replacement is drawn', () => {
  // Suppressing WebKit's overlay indicator on a scroller with nothing in its
  // place would leave that surface with no scroll feedback at all. The CSS is
  // therefore scoped to `.has-scroll-indicator`, and anything that opts in must
  // render the replacement.
  // Raw, not stripped: postcss parses comments into their own nodes, so the
  // declaration scan below never sees the prose that explains it.
  const mobileCss = read('styles/mobile.css');

  it('scopes every transcript scrollbar suppression to a wrap that carries the replacement', () => {
    expect(transcriptSuppressions().length).toBeGreaterThan(0);
    for (const rule of transcriptSuppressions()) {
      expect(rule.selector).toContain('has-scroll-indicator');
    }
  });

  it('suppresses the native scrollbar only for the input class the replacement serves', () => {
    // The replacement is summoned by isUserScrolling(), which keys off
    // `touchmove` and so says nothing about a wheel or an arrow key. Gating the
    // suppression on viewport WIDTH alone would strip the native scrollbar from
    // a narrow window driven by a mouse and never summon the replacement, which
    // leaves that user with no scroll feedback at all.
    for (const rule of transcriptSuppressions()) {
      expect(rule.atRules).toMatch(/hover:\s*none/);
      expect(rule.atRules).toMatch(/pointer:\s*coarse/);
    }
  });

  it('turns the replacement on under exactly the gate that suppresses the native one', () => {
    const shown = cssRules(mobileCss).filter(
      r => r.selector.includes('.thread-scroll-indicator') && /display:\s*block/.test(r.body),
    );
    expect(shown.length).toBeGreaterThan(0);

    const gates = new Set(transcriptSuppressions().map(r => r.atRules));
    expect(gates.size).toBe(1);
    for (const rule of shown) {
      expect(rule.atRules).toBe([...gates][0]);
    }
  });

  /** The rules that strip the transcript's native scrollbar. */
  function transcriptSuppressions() {
    return cssRules(mobileCss).filter(
      r =>
        r.selector.includes('.thread-content') &&
        (r.selector.includes('::-webkit-scrollbar') || /scrollbar-width:\s*none/.test(r.body)),
    );
  }

  it('renders the replacement in every component that opts into the suppression', () => {
    const optedIn = tsxFiles(src).filter(f => readFileSync(f, 'utf8').includes('has-scroll-indicator'));
    expect(optedIn.length).toBeGreaterThan(0);
    for (const file of optedIn) {
      expect(readFileSync(file, 'utf8')).toContain('class="thread-scroll-indicator"');
    }
  });
});

/** Every `.tsx` under `src`, excluding test files. */
function tsxFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'node_modules') continue;
      out.push(...tsxFiles(full));
    } else if (entry.name.endsWith('.tsx') && !entry.name.includes('.test.')) {
      out.push(full);
    }
  }
  return out;
}
