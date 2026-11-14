/**
 * An edited dashboard tile drops its clip, so the remove badge can hang past
 * its corner. The widget box's scroller must then clip the frame to the
 * rounded corner, or its square iframe corners show through the dashed outline.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { cssRules } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const dashboard = cssRules(readFileSync(resolve(here, '..', 'panels/dashboard.css'), 'utf-8'));

function props(selector: string): Map<string, string> {
  const found = dashboard.filter(r => r.selector === selector && r.atRules === '');
  expect(found, `expected one rule for ${selector}`).toHaveLength(1);
  return found[0].props;
}

describe('an edited dashboard tile keeps its rounded corners', () => {
  it('lets the badge out of the tile', () => {
    expect(props('.dashboard-tile.is-editing').get('overflow')).toBe('visible');
  });

  it('clips the frame to the corner inside the border', () => {
    const frame = props('.dashboard-tile.is-editing > .widget-box-scroll');
    expect(frame.get('overflow')).toBe('hidden');
    expect(frame.get('border-radius')).toBe('calc(var(--radius-surface) - 1px)');
  });
});
