/**
 * The file preview's two line-wrapping modes.
 *
 * A line wider than the viewer used to be clipped, with no wrap and no visible
 * way to reach its tail. The fix is a mode pair on the shared renderer
 * (`LineNumberedCode`). Each mode rests on a handful of declarations that look
 * cosmetic and are not. A source scan rather than a browser test, matching the
 * sibling geometry guards: `e2e/file-preview-long-lines.spec.ts` drives the
 * rendered behaviour, this pins the declarations it rests on.
 *
 * Full rationale: docs/plans/2026-09-15-file-preview-long-line-wrapping.md.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { cssRules, rulesTargeting, selectorList, styleSheetPaths, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const CONTENT: string = readFileSync(resolve(here, '../panels/content.css'), 'utf8');
const COMPONENTS: string = readFileSync(resolve(here, '../components.css'), 'utf8');
const PREVIEWS: string = readFileSync(resolve(here, '../panels/previews.css'), 'utf8');

/** The one rule whose selector contains `fragment`, among those styling
 *  `className`. */
function ruleWith(css: string, className: string, fragment: string): CssRule {
  const hits = rulesTargeting(css, className).filter(r => r.selector.includes(fragment));
  expect(hits, `expected one "${fragment} .${className}" rule`).toHaveLength(1);
  return hits[0];
}

describe('soft wrap, the mode a preview opens on', () => {
  const wrapped = ruleWith(CONTENT, 'line-content', 'line-numbered-wrap');

  it('wraps the content cell, so no tail is off the right edge', () => {
    expect(wrapped.props.get('white-space')).toBe('pre-wrap');
  });

  // `break-word` does not lower a flex item's min-content size, and the
  // automatic minimum is measured against exactly that. One unbreakable token
  // would still push the row wider than the view.
  it('breaks anywhere, which is what a flex item measures its floor against', () => {
    expect(wrapped.props.get('overflow-wrap')).toBe('anywhere');
    expect(wrapped.props.get('min-width')).toBe('0');
  });

  // A side-by-side diff column stays aligned row for row because every row is
  // exactly one line tall. Wrapping there would drift the two sides apart, so
  // the mode is opt-in and the bare rule must never carry it.
  it('leaves the bare rule unwrapped, for the diff columns that share it', () => {
    const bare = rulesTargeting(CONTENT, 'line-content')
      .filter(r => !r.selector.includes('line-numbered-'));
    expect(bare).toHaveLength(1);
    expect(bare[0].props.get('white-space')).toBe('pre');
  });
});

/** A sticky box per line made a phone pan stutter: WebKit repositions every
 *  sticky box on each scroll frame, so the cost grew with the file. Measured in
 *  WebKit mobile emulation, a 2,574-line file panned at 48 ms a frame and an
 *  8,000-line one at 177 ms. With one pinned column both run at 16.7 ms.
 *  Full numbers: docs/plans/2026-10-07-line-numbered-pan-single-gutter.md. */
describe('pan pins ONE gutter column, never a box per line', () => {
  const PER_LINE = /\.(code-line|line-number|line-content|line-selected)\b/;
  const stylesRoot: string = resolve(here, '..');

  it('makes no per-line element sticky in any stylesheet', () => {
    const offenders: string[] = [];
    for (const path of styleSheetPaths(stylesRoot)) {
      for (const rule of cssRules(readFileSync(path, 'utf8'))) {
        if (rule.props.get('position') !== 'sticky') continue;
        // The subject of the selector is its last compound.
        for (const sel of selectorList(rule.selector)) {
          const subject = sel.trim().split(/\s+|>|\+|~/).filter(Boolean).pop() ?? '';
          if (PER_LINE.test(subject)) offenders.push(`${path.split('/styles/')[1]}: ${sel}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it('pins the gutter column to the left edge of the scroll container', () => {
    const gutter = cssRules(CONTENT).filter(r => r.selector === '.line-numbered-gutter');
    expect(gutter).toHaveLength(1);
    expect(gutter[0].props.get('position')).toBe('sticky');
    expect(gutter[0].props.get('left')).toBe('0');
    // Opaque, since the code pans beneath it.
    expect(gutter[0].props.get('background')).toBe('var(--code-surface)');
  });

  it('lays the two columns side by side, sized to the widest row', () => {
    const pre = rulesTargeting(CONTENT, 'line-numbered-pan')
      .filter(r => r.selector === '.line-numbered-pan');
    expect(pre).toHaveLength(1);
    expect(pre[0].props.get('display')).toBe('flex');
    expect(pre[0].props.get('width')).toBe('max-content');
    expect(pre[0].props.get('min-width')).toBe('100%');
  });

  // Cell N and row N stay level only because both are one line of the same
  // height. One declaration for both, so the two cannot drift apart.
  it('gives a gutter cell the same line height as a code row', () => {
    const shared = cssRules(CONTENT).filter(r => r.props.has('line-height')
      && selectorList(r.selector).includes('.code-line')
      && selectorList(r.selector).includes('.line-numbered-gutter .line-number'));
    expect(shared).toHaveLength(1);
  });

  // No pan rule repaints a row: with nothing pinned over the code, the base
  // translucent `.line-selected` tint is the right one.
  it('leaves the code rows and their tint to the base rules', () => {
    const repaints = cssRules(CONTENT)
      .filter(r => r.selector.includes('line-numbered-pan') && r.props.has('background'));
    expect(repaints.map(r => r.selector)).toEqual([]);
  });

  // A scroll container's inline padding is inside its scrollport, so panned
  // code slides through the strip left of the gutter. Without the bleed the
  // gutter is pinned and code still passes beside it.
  it('paints over the scroll container padding the code pans through', () => {
    const bleed = cssRules(CONTENT)
      .filter(r => r.selector === '.line-numbered-gutter::before');
    expect(bleed, 'the gutter needs a ::before bleeding over the container inset').toHaveLength(1);
    expect(bleed[0].props.get('width')).toBe('var(--code-gutter-inset, 0px)');
    expect(bleed[0].props.get('right')).toBe('100%');
    expect(bleed[0].props.get('background')).toBe('inherit');
  });
});

/** Find in file walks text nodes, so a number held as text would match a
 *  numeric search. The markup carries it as an attribute and this rule draws
 *  it (`line-numbered-pan-layout.test.tsx` pins the markup half). */
describe('the line number is drawn, not written', () => {
  it('generates the number from its attribute', () => {
    const drawn = cssRules(CONTENT).filter(r => r.selector === '.line-number::before');
    expect(drawn).toHaveLength(1);
    expect(drawn[0].props.get('content')).toBe('attr(data-line-number)');
  });
});

/** The bleed is only as wide as the container says, so each scroll container
 *  must name its own inline padding. Written THROUGH the var in both, so the
 *  padding and the bleed cannot drift apart. */
describe('every file-preview scroll container names its inset', () => {
  const inset = (css: string, className: string) => {
    const rules = rulesTargeting(css, className).filter(r => r.atRules === '');
    expect(rules, `${className} must be declared once at the top level`).toHaveLength(1);
    return rules[0].props;
  };

  it('the data-file preview', () => {
    const props = inset(PREVIEWS, 'file-preview-content');
    expect(props.get('--code-gutter-inset')).toBe('0.75rem');
    expect(props.get('padding')).toBe('var(--code-gutter-inset)');
  });

  it('the repository-file preview', () => {
    const props = inset(CONTENT, 'repo-file-content');
    expect(props.get('--code-gutter-inset')).toBe('1.25rem');
    expect(props.get('padding')).toBe('1rem var(--code-gutter-inset)');
  });
});

/** The third mode carries no class at all. So no rule of either mode reaches a
 *  side-by-side diff column, whose rows carry their own single-class tints. */
describe('a caller-owned block is reached by neither mode', () => {
  const MODE = /\.line-numbered-(wrap|pan)\b/;

  it('never styles a diff column, a tint, or a filler row', () => {
    const reaching = cssRules(CONTENT)
      .filter(r => MODE.test(r.selector))
      .filter(r => /side-by-side/.test(r.selector));
    expect(reaching.map(r => r.selector)).toEqual([]);
  });

  it('leaves the diff column tints on their own single class', () => {
    for (const cls of ['side-by-side-diff-addition', 'side-by-side-diff-deletion', 'side-by-side-diff-filler']) {
      const rules = rulesTargeting(CONTENT, cls);
      expect(rules, cls).toHaveLength(1);
      expect(rules[0].selector, cls).toBe(`.${cls}`);
    }
  });
});

describe('the surface the gutter paints', () => {
  it('defaults to the content pane it sits on', () => {
    const base = rulesTargeting(CONTENT, 'line-numbered')
      .filter(r => r.selector === '.line-numbered');
    expect(base).toHaveLength(1);
    expect(base[0].props.get('--code-surface')).toBe('var(--bg-primary)');
  });

  // The modal is a `.surface` on --surface-bg, not the content pane's fill, so
  // the default would paint a band in the wrong colour. Declared on the same
  // element, one step more specific, so source order cannot decide it.
  it('follows the preview modal onto its own panel colour', () => {
    const inModal = ruleWith(COMPONENTS, 'line-numbered', 'file-preview-modal');
    expect(inModal.props.get('--code-surface')).toBe('var(--surface-bg)');
  });
});
