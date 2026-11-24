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

  it('refuses to shrink the header below its own content', () => {
    const min = settled('whats-new-release-header', 'min-width') ?? 'auto';
    expect(min).not.toMatch(/^0(px|rem|%)?$/);
  });

  it('keeps a date on one line', () => {
    expect(settled('whats-new-date', 'white-space')).toBe('nowrap');
  });

  it('holds the control at the trailing edge once it wraps', () => {
    expect(settled('whats-new-release-action', 'margin-left')).toBe('auto');
  });
});
