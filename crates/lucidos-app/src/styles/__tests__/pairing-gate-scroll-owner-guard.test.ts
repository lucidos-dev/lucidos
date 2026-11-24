import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const PICKER_CSS = readFileSync(resolve(here, '../picker.css'), 'utf8');

function settled(className: string, prop: string): string | undefined {
  let value: string | undefined;
  for (const rule of rulesTargeting(PICKER_CSS, className)) {
    const own = rule.props.get(prop);
    if (own !== undefined) value = own;
  }
  return value;
}

describe('pairing gate scroll ownership', () => {
  it('gives the gate a viewport-sized box that scrolls', () => {
    expect(settled('pairing-gate', 'overflow-y'), 'the phone document cannot scroll').toBe('auto');
    expect(settled('pairing-gate', 'position')).toBe('fixed');
    expect(settled('pairing-gate', 'inset')).toBe('0');
  });

  it('centres the column by auto margins, never by the gate', () => {
    expect(settled('pairing-column', 'margin')).toBe('auto');
    expect(settled('pairing-gate', 'align-items'), 'this strands the column top').toBeUndefined();
  });

  it('clears the safe area on every side', () => {
    const padding = settled('pairing-gate', 'padding') ?? '';
    for (const side of ['top', 'right', 'bottom', 'left']) {
      expect(padding, `the ${side} gutter ignores the safe area`).toContain(
        `var(--safe-area-${side})`,
      );
    }
  });
});
