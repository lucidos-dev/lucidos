/**
 * The pairing gate scrolls itself, and its column starts at the top when it is
 * too tall to fit.
 *
 * `mobile.css` sets `html { overflow: hidden }` under 768px, so a phone
 * document does not scroll. Every full-screen surface therefore owns its own
 * scroll container: `.ws-picker` already did, `.pairing-gate` did not, and its
 * install recipe was clipped with no way to reach the rest.
 *
 * Two declarations make the box scroll and neither works alone. `overflow-y`
 * scrolls nothing on an auto-height box, and a viewport-sized box without it
 * clips. So both are asserted, along with the absence of the cross-axis
 * centring that would strand the column's first lines above the scroll origin.
 *
 * A source scan rather than a browser test: no emulator reproduces the phone
 * rule that causes it, for the same reason
 * `transcript-fade-scroll-gutter-guard.test.ts` is one.
 */
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

/** The last value any rule in the sheet gives `prop` on `className`. */
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
    // Without a constrained height the box grows to its content and scrolls
    // nothing, whatever `overflow-y` says.
    expect(settled('pairing-gate', 'position')).toBe('fixed');
    expect(settled('pairing-gate', 'inset')).toBe('0');
  });

  it('centres the column by auto margins, never by the gate', () => {
    expect(settled('pairing-column', 'margin')).toBe('auto');
    // `align-items: center` overflows an over-tall column above the scroll
    // origin, which no drag can reach.
    expect(settled('pairing-gate', 'align-items'), 'this strands the column top').toBeUndefined();
  });

  // The page is `viewport-fit=cover` under a translucent status bar, and this
  // screen renders instead of the shell, so no other rule insets it. A flat
  // gutter puts the title under the clock and the Pair button under the home
  // indicator.
  it('clears the safe area on every side', () => {
    const padding = settled('pairing-gate', 'padding') ?? '';
    for (const side of ['top', 'right', 'bottom', 'left']) {
      expect(padding, `the ${side} gutter ignores the safe area`).toContain(
        `env(safe-area-inset-${side}`,
      );
    }
  });
});
