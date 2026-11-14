/**
 * The step-detail code blocks scroll inside themselves, so the body around
 * them must not scroll too. The body is a grid whose rows share the height.
 * A scroll box's automatic minimum height is 0, so long blocks split the room
 * and a short one keeps its full height. A flex column cannot do this,
 * because a block's flex base size ignores its max-height cap.
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
const rules = cssRules(steps).filter(r => r.atRules === '');
const prop = (selector: string, name: string) => rules.find(r => r.selector === selector)?.props.get(name);

describe('the step-detail code blocks share the body height', () => {
  it('lays the body out as a top-packed grid', () => {
    expect(prop('.step-detail-body', 'display')).toBe('grid');
    expect(prop('.step-detail-body', 'align-content')).toBe('start');
  });

  it('lets a fetched code block give way like a bare one, down to the block inside', () => {
    expect(prop('.step-detail-fade', 'min-height')).toBeUndefined();
    expect(prop('.step-detail-fade:has(pre)', 'min-height')).toBe('0');
    expect(prop('.step-detail-fade', 'grid-template-rows')).toBe('minmax(0, 1fr)');
    expect(prop('.step-detail-fade .loading-fade-content', 'min-height')).toBe('0');
  });
});
