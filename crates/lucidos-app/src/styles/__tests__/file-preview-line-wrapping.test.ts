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

  it('breaks anywhere, which is what a flex item measures its floor against', () => {
    expect(wrapped.props.get('overflow-wrap')).toBe('anywhere');
    expect(wrapped.props.get('min-width')).toBe('0');
  });

  it('leaves the bare rule unwrapped, for the diff columns that share it', () => {
    const bare = rulesTargeting(CONTENT, 'line-content')
      .filter(r => !r.selector.includes('line-numbered-'));
    expect(bare).toHaveLength(1);
    expect(bare[0].props.get('white-space')).toBe('pre');
  });
});

describe('pan pins ONE gutter column, never a box per line', () => {
  const PER_LINE = /\.(code-line|line-number|line-content|line-selected)\b/;
  const stylesRoot: string = resolve(here, '..');

  it('makes no per-line element sticky in any stylesheet', () => {
    const offenders: string[] = [];
    for (const path of styleSheetPaths(stylesRoot)) {
      for (const rule of cssRules(readFileSync(path, 'utf8'))) {
        if (rule.props.get('position') !== 'sticky') continue;
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

  it('gives a gutter cell the same line height as a code row', () => {
    const shared = cssRules(CONTENT).filter(r => r.props.has('line-height')
      && selectorList(r.selector).includes('.code-line')
      && selectorList(r.selector).includes('.line-numbered-gutter .line-number'));
    expect(shared).toHaveLength(1);
  });

  it('leaves the code rows and their tint to the base rules', () => {
    const repaints = cssRules(CONTENT)
      .filter(r => r.selector.includes('line-numbered-pan') && r.props.has('background'));
    expect(repaints.map(r => r.selector)).toEqual([]);
  });

  it('paints over the scroll container padding the code pans through', () => {
    const bleed = cssRules(CONTENT)
      .filter(r => r.selector === '.line-numbered-gutter::before');
    expect(bleed, 'the gutter needs a ::before bleeding over the container inset').toHaveLength(1);
    expect(bleed[0].props.get('width')).toBe('var(--code-gutter-inset, 0px)');
    expect(bleed[0].props.get('right')).toBe('100%');
    expect(bleed[0].props.get('background')).toBe('inherit');
  });
});

describe('the line number is drawn, not written', () => {
  it('generates the number from its attribute', () => {
    const drawn = cssRules(CONTENT).filter(r => r.selector === '.line-number::before');
    expect(drawn).toHaveLength(1);
    expect(drawn[0].props.get('content')).toBe('attr(data-line-number)');
  });
});

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

  it('follows the preview modal onto its own panel colour', () => {
    const inModal = ruleWith(COMPONENTS, 'line-numbered', 'file-preview-modal');
    expect(inModal.props.get('--code-surface')).toBe('var(--surface-bg)');
  });
});
