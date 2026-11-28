import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const STYLES_ROOT: string = resolve(here, '..');

const SCROLLERS = [
  'thread-content',
  'content-pane-body',
  'thread-drawer-list',
] as const;

describe('scroll containers stay hit targets', () => {
  const offenders: string[] = [];
  for (const path of styleSheetPaths(STYLES_ROOT)) {
    const css: string = readFileSync(path, 'utf8');
    for (const scroller of SCROLLERS) {
      for (const rule of rulesTargeting(css, scroller)) {
        if (rule.props.get('pointer-events') !== 'none') continue;
        offenders.push(`${relative(STYLES_ROOT, path)}: ${rule.atRules} { ${rule.selector} }`);
      }
    }
  }

  it('never sets pointer-events: none on the scroller itself', () => {
    expect(
      offenders,
      'A scroller taken out of hit-testing cannot be panned: the touch resolves to whatever '
      + 'sits behind it, which scrolls nothing. To block stray taps, target the children '
      + '(`.scroller > *`) instead. `pointer-events` inherits, so the subtree still goes inert '
      + 'while the pan keeps landing on the scroller.',
    ).toEqual([]);
  });
});
