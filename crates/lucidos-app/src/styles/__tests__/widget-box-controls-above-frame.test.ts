/**
 * A widget box, a thread card's or a dashboard tile's, draws its fade and the
 * tile's menu and edit badges over the widget's frame. Tree order alone does
 * not keep them on top on iOS: a frame whose widget composites (an animated
 * transform, as Thread cost's bars have) paints over its later siblings. The
 * menu then still takes a tap but cannot be seen. So the frame takes the
 * bottom layer and every other child stacks above it. The box isolates those
 * z-indices from the rest of the app.
 */
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

/** The one top-level rule with exactly this selector. */
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
