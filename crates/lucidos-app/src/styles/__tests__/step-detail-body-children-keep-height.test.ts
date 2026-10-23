/**
 * The step-detail body is a flex column that scrolls, and its code blocks
 * scroll too. A scroll box's automatic minimum height is 0, so a tall sibling
 * (the checkpoint's diff) once squeezed the command box to a sliver. The body
 * is the scroller, so no child of it may shrink.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const steps: string = readFileSync(resolve(here, '../steps.css'), 'utf8');

describe('a step-detail body child keeps its own height', () => {
  it('stops every direct child of the body from shrinking', () => {
    const rule = cssRules(steps).find(r => r.selector === '.step-detail-body > *' && r.atRules === '');
    expect(rule?.props.get('flex-shrink')).toBe('0');
  });
});
