/**
 * The Canvas header's icons regroup in one frame when the pane resizes.
 *
 * A `left` transition on the ⋯ trigger made it trail the pointer on resize.
 * At the compact breakpoint it also glided across the whole row. A `width`
 * transition on the Refresh slot eased beside that glide. Both read as the
 * icons dancing, so neither may come back.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const shellCss: string = readFileSync(resolve(here, '../panels/shell.css'), 'utf-8');

describe('the Canvas header regroups its icons in one frame', () => {
  for (const control of ['content-header-more', 'content-refresh-btn']) {
    it(`gives .${control} no geometry transition`, () => {
      const rules = rulesTargeting(shellCss, control);
      expect(rules.length, `no rule styles .${control}`).toBeGreaterThan(0);
      for (const r of rules) {
        for (const prop of ['transition', 'transition-property']) {
          const value = r.props.get(prop) ?? '';
          expect(value, `${r.selector} eases its geometry`).not.toMatch(/\b(left|width|all)\b/);
        }
      }
    });
  }
});
