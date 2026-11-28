import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { cssRules, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const widgets = cssRules(readFileSync(resolve(here, '..', 'chat/widgets.css'), 'utf-8'));

function rule(selector: string): CssRule {
  const found = widgets.filter(r => r.selector === selector && r.atRules === '');
  expect(found, `expected one rule for ${selector}`).toHaveLength(1);
  return found[0];
}

describe('a widget box keeps its controls above its frame', () => {
  it('isolates its own stacking, so the layers stay inside the box', () => {
    expect(rule('.widget-box').props.get('isolation')).toBe('isolate');
  });

  it('puts the frame on the bottom layer and every other child above it', () => {
    const frame = Number(rule('.widget-box > .widget-box-scroll').props.get('z-index'));
    const controls = Number(rule('.widget-box > :not(.widget-box-scroll)').props.get('z-index'));
    expect(controls).toBeGreaterThan(frame);
  });
});
