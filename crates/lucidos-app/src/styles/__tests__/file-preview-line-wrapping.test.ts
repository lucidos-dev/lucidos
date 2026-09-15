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

import { cssRules, rulesTargeting, type CssRule } from './css-rule-helpers';

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

describe('pan, where the code slides under a pinned gutter', () => {
  it('pins the gutter to the left edge of the scroll container', () => {
    const gutter = ruleWith(CONTENT, 'line-number', 'line-numbered-pan');
    expect(gutter.props.get('position')).toBe('sticky');
    expect(gutter.props.get('left')).toBe('0');
    // A sticky cell paints over whatever passes beneath it, and `inherit` is
    // what makes it wear the row's own background. So the selected line's tint
    // stays unbroken across the gutter instead of showing a plain band.
    expect(gutter.props.get('background')).toBe('inherit');
  });

  it('sizes every row to the widest, so a highlight spans the scroll width', () => {
    const pre = rulesTargeting(CONTENT, 'line-numbered-pan')
      .filter(r => r.selector === '.line-numbered-pan');
    expect(pre).toHaveLength(1);
    expect(pre[0].props.get('width')).toBe('max-content');
    expect(pre[0].props.get('min-width')).toBe('100%');
  });

  it('gives the row an opaque surface for the gutter to inherit', () => {
    const row = ruleWith(CONTENT, 'code-line', 'line-numbered-pan');
    expect(row.props.get('background')).toBe('var(--code-surface)');
  });

  // The base tint mixes the accent over `transparent`, which a sticky gutter
  // cannot paint with. This mode restates it over the surface instead.
  it('restates the selection tint opaquely', () => {
    const selected = ruleWith(CONTENT, 'line-selected', 'line-numbered-pan');
    expect(selected.props.get('background')).toContain('var(--code-surface)');
    expect(selected.props.get('background')).not.toContain('transparent');
  });

  // A scroll container's inline padding is inside its scrollport, so panned
  // code slides through the strip left of the gutter. Without the bleed the
  // gutter is pinned and code still passes beside it.
  it('paints over the scroll container padding the code pans through', () => {
    const bleed = cssRules(CONTENT)
      .filter(r => r.selector === '.line-numbered-pan .line-number::before');
    expect(bleed, 'the gutter needs a ::before bleeding over the container inset').toHaveLength(1);
    expect(bleed[0].props.get('width')).toBe('var(--code-gutter-inset, 0px)');
    expect(bleed[0].props.get('right')).toBe('100%');
    expect(bleed[0].props.get('background')).toBe('inherit');
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

/** The third mode carries no class at all, which is what keeps `pan`'s opaque
 *  row surface off a side-by-side diff column. That surface is two classes
 *  deep and would outrank every one of the column's own single-class tints. */
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

  // The modal is a --bg-secondary panel, so the default would paint a band in
  // the wrong grey. Declared on the same element, one step more specific, so
  // source order cannot decide it.
  it('follows the preview modal onto its own panel colour', () => {
    const inModal = ruleWith(COMPONENTS, 'line-numbered', 'file-preview-modal');
    expect(inModal.props.get('--code-surface')).toBe('var(--bg-secondary)');
  });
});
