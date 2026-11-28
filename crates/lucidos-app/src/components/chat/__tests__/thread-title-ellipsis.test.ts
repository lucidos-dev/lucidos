import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, styleSheetPaths, type CssRule } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesRoot: string = resolve(here, '../../../styles');
const drawerCss = readFileSync(resolve(stylesRoot, 'drawer.css'), 'utf-8');

function ruleBody(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const m = css.match(new RegExp(`(^|})\\s*${escaped}\\s*\\{([^}]*)\\}`, 'm'));
  if (!m) throw new Error(`no rule for selector: ${selector}`);
  return m[2];
}

function rulesFor(className: string): CssRule[] {
  return styleSheetPaths(stylesRoot)
    .flatMap(path => rulesTargeting(readFileSync(path, 'utf-8'), className));
}

const CLIPS = /^(hidden|clip|auto|scroll)$/;

describe('Desktop header title truncates with an ellipsis', () => {
  const base = ruleBody(drawerCss, '.thread-title');
  const desktop = ruleBody(drawerCss, '.thread-view-header .thread-title');
  const text = ruleBody(drawerCss, '.thread-view-header .thread-title-text');

  it('shrinks below its text, so the text can overflow it', () => {
    expect(base).toMatch(/min-width:\s*0/);
    expect(desktop).toMatch(/flex:\s*0 1 auto/);
    expect(desktop).toMatch(/max-width:\s*calc\(100% - var\(--title-icon-box\)/);
    expect(desktop).not.toMatch(/width:\s*max-content/);
  });

  it('carries the ellipsis on the inner text span and stays on one line', () => {
    expect(text).toMatch(/display:\s*block/);
    expect(text).toMatch(/text-overflow:\s*ellipsis/);
    expect(text).toMatch(/white-space:\s*nowrap/);
    expect(text).toMatch(/overflow:\s*hidden/);
  });

  it('clips on a square box, never on the rounded button', () => {
    for (const className of ['thread-title', 'thread-title-menu']) {
      for (const rule of rulesFor(className)) {
        const overflow = rule.props.get('overflow') ?? rule.props.get('overflow-x') ?? 'visible';
        expect(overflow, `${rule.selector} clips a box that rounds its corners`).not.toMatch(CLIPS);
      }
    }
    for (const rule of rulesFor('thread-title-text')) {
      expect(rule.props.has('border-radius'), `${rule.selector} rounds the clipping span`).toBe(false);
    }
  });
});
