/** The Memory tree shows depth by indent alone, with no guide rail. */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const stylesRoot: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sheets = styleSheetPaths(stylesRoot).map((path) => ({ path, css: readFileSync(path, 'utf-8') }));

const RAIL_PROPS = ['border-left', 'border-left-width', 'border-left-style', 'border-inline-start', 'border'];

describe('the Memory tree indents without rails', () => {
  it('no rule draws a rail down a line\'s children', () => {
    for (const { path, css } of sheets) {
      for (const rule of rulesTargeting(css, 'tree-line-children')) {
        for (const prop of RAIL_PROPS) {
          expect(rule.props.get(prop), `${path}: ${rule.selector} sets ${prop}`).toBeUndefined();
        }
      }
    }
  });

  it('indents a line\'s children', () => {
    const indents = sheets
      .flatMap(({ css }) => rulesTargeting(css, 'tree-line-children'))
      .map((rule) => rule.props.get('padding-left'))
      .filter(Boolean);
    expect(indents.length).toBeGreaterThan(0);
  });
});
