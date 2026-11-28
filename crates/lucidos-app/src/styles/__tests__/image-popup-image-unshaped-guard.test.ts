import { describe, it, expect } from 'vitest';
import { cssRules, selectorList, styleSheetPaths } from './css-rule-helpers';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here: string = dirname(fileURLToPath(import.meta.url));
const STYLES: string = resolve(here, '..');
const BANNED = ['border-radius', 'clip-path', 'filter'];

function stylesPopupImage(one: string): boolean {
  if (!/\bimage-popup\b/.test(one)) return false;
  const compounds = one.trim().split(/[\s>+~]+/);
  return /^img(?![\w-])/.test(compounds[compounds.length - 1] ?? '');
}

describe('the image popup does not reshape the image', () => {
  it('no sheet gives the popup image a radius, a clip or a filter', () => {
    const offenders: string[] = [];
    for (const path of styleSheetPaths(STYLES)) {
      const css: string = readFileSync(path, 'utf-8');
      for (const rule of cssRules(css)) {
        if (!selectorList(rule.selector).some(stylesPopupImage)) continue;
        for (const prop of BANNED) {
          const value = rule.props.get(prop);
          if (value !== undefined) offenders.push(`${path}: ${rule.selector} { ${prop}: ${value} }`);
        }
      }
    }
    expect(offenders, offenders.join('\n')).toEqual([]);
  });
});
