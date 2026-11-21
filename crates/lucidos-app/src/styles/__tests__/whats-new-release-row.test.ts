/**
 * Every release row reads the same on its first line: chevron, version, date,
 * chip at the trailing edge. The control wraps below it where the two cannot
 * share a line.
 *
 * The reported shape was a row whose date broke into `2026-` and `09-15` while
 * the row under it sat on one line. Two things caused it together, so both are
 * pinned: the header could shrink to nothing (`min-width: 0`), and the date was
 * free to break at its own hyphens. Neither is enough alone, since a date held
 * on one line still squeezes the row when the header may shrink under it.
 *
 * A source scan rather than a browser test. The wrap point depends on the pane
 * width and on the user's UI scale, so no fixed viewport is the case that
 * matters. What a test can hold is which declarations are written.
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
const css = readFileSync(resolve(here, '../settings/whats-new.css'), 'utf-8');

/** The last value any rule in the sheet gives `prop` on `className`'s own box.
 *  Last, not first: a later rule overriding it is exactly the regression these
 *  assertions have to see. */
function settled(className: string, prop: string): string | null {
  const values = rulesTargeting(css, className)
    .map((rule) => rule.props.get(prop))
    .filter((v): v is string => v !== undefined);
  return values[values.length - 1] ?? null;
}

describe('the What’s New release row', () => {
  it('lets the control wrap instead of squeezing the line above it', () => {
    expect(settled('whats-new-release-row', 'flex-wrap')).toBe('wrap');
  });

  // `0` is what let the date break: it let the header shrink to nothing while
  // the action held its width. The assertion is against that value rather than
  // for one, because `auto` and a deleted declaration are the same behaviour
  // and either is correct.
  it('refuses to shrink the header below its own content', () => {
    const min = settled('whats-new-release-header', 'min-width') ?? 'auto';
    expect(min).not.toMatch(/^0(px|rem|%)?$/);
  });

  it('keeps a date on one line', () => {
    expect(settled('whats-new-date', 'white-space')).toBe('nowrap');
  });

  // A wrapped control sits under the chip it follows, not under the chevron.
  it('holds the control at the trailing edge once it wraps', () => {
    expect(settled('whats-new-release-action', 'margin-left')).toBe('auto');
  });
});
