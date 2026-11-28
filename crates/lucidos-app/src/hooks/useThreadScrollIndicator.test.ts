import { describe, it, expect, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

vi.mock('../components/chat/scrollState', () => ({
  classifyScroll: () => ({ navigation: false }),
}));

import { INDICATOR_HIDE_DELAY_MS } from './useThreadScrollIndicator';
import { cssRules, rulesTargeting, styleSheetPaths } from '../styles/__tests__/css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, '..');
const read = (rel: string) => readFileSync(resolve(src, rel), 'utf8');

function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/^\s*\/\/.*$/gm, ' ');
}

const hookCode = stripComments(read('hooks/useThreadScrollIndicator.ts'));

describe('the per-scroll path stays composited', () => {
  it('writes only composited or paint-only properties as inline styles', () => {
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
    expect(hookCode).not.toMatch(/setProperty\s*\(/);
    expect(hookCode).not.toMatch(/documentElement/);
  });
});

describe('the fade is a post-motion linger, not a scroll budget', () => {
  it('is long enough to bridge the gaps between momentum frames', () => {
    expect(INDICATOR_HIDE_DELAY_MS).toBeGreaterThan(500);
  });

  it('does not outstay the scroll it belongs to', () => {
    expect(INDICATOR_HIDE_DELAY_MS).toBeLessThan(3000);
  });

  it('does not depend on the touch window, which no longer bounds visibility', () => {
    expect(hookCode).not.toMatch(/USER_SCROLL_WINDOW_MS/);
  });
});

describe('the thumb box is measured with the right API for each axis', () => {
  it('reads the bar width at sub-pixel precision, never through offsetWidth', () => {
    expect(hookCode).not.toMatch(/offsetWidth/);
    expect(hookCode).toMatch(/getBoundingClientRect\(\)\.width/);
  });

  it('reads the scale base through offsetHeight, which the transform does not touch', () => {
    expect(hookCode).toMatch(/offsetHeight/);
    expect(hookCode).not.toMatch(/getBoundingClientRect\(\)\.height/);
  });
});

describe('the visibility rule lives in one testable place', () => {
  it('routes every scroll event through nextIndicatorVisibility', () => {
    expect(hookCode).toMatch(/nextIndicatorVisibility\(/);
    expect(hookCode).not.toMatch(/if\s*\(\s*isUserScrolling\(\)\s*\)/);
  });
});

describe('the native indicator is only suppressed where a replacement is drawn', () => {
  const mobileCss = read('styles/mobile.css');

  it('scopes every transcript scrollbar suppression to a wrap that carries the replacement', () => {
    expect(transcriptSuppressions().length).toBeGreaterThan(0);
    for (const rule of transcriptSuppressions()) {
      expect(rule.selector).toContain('has-scroll-indicator');
    }
  });

  it('suppresses the native scrollbar only for the input class the replacement serves', () => {
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

describe('the parked thumb tracks the chrome above the transcript', () => {
  const mobileCss = read('styles/mobile.css');

  it('derives the track top from the header and title bar heights', () => {
    const positioned = rulesTargeting(mobileCss, 'thread-scroll-indicator').filter(r =>
      r.props.has('top'),
    );
    expect(positioned.length).toBe(1);
    const top = positioned[0].props.get('top')!;
    for (const chrome of ['--mobile-header-height', '--mobile-thread-title-height']) {
      expect(top).toContain(chrome);
    }
  });
});

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

describe('desktop keeps the native scrollbar (ADR 0258)', () => {
  const rules = styleSheetPaths(resolve(src, 'styles')).flatMap(f => cssRules(readFileSync(f, 'utf8')));

  it('never hides the transcript\'s native thumb', () => {
    const thumbRules = rules.filter(
      r => r.selector.includes('.thread-content') && r.selector.includes('::-webkit-scrollbar-thumb'),
    );
    for (const rule of thumbRules) {
      expect(rule.props.get('background'), rule.selector).not.toBe('transparent');
      expect(rule.props.get('display'), rule.selector).not.toBe('none');
    }
  });

  it('draws the indicator only under the touch gate', () => {
    const shown = rules.filter(
      r => r.selector.includes('.thread-scroll-indicator') && r.props.get('display') === 'block',
    );
    expect(shown.length).toBeGreaterThan(0);
    for (const rule of shown) {
      expect(rule.atRules).toMatch(/hover:\s*none/);
      expect(rule.atRules).toMatch(/pointer:\s*coarse/);
    }
  });
});
