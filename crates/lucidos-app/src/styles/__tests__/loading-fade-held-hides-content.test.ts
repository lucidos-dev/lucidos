/**
 * While LoadingFade holds its skeleton through the minimum-visible floor, the
 * loaded content must not draw beside it. The wrapper carries `loading-fade-held`
 * and this rule hides the content wrapper, keeping its layout so the cell does not
 * resize when the floor lifts.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const componentsCss: string = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), '../components.css'),
  'utf-8',
);

describe('loading-fade-held', () => {
  it('hides the content wrapper without removing it from layout', () => {
    const rule = rulesTargeting(componentsCss, 'loading-fade-content').find(r =>
      r.selector.includes('.loading-fade-held'),
    );
    expect(rule, 'no rule hides held content').toBeDefined();
    expect(rule!.props.get('visibility')).toBe('hidden');
    expect(rule!.props.has('display')).toBe(false);
  });
});
