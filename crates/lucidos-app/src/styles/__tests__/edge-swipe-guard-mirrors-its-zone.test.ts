/**
 * The edge-nav guards must be exactly as wide as the strips they cover.
 *
 * `.edge-swipe-left` / `.edge-swipe-right` are transparent strips that sit
 * above app iframes so a touch there reaches the parent swipe handler.
 * `shouldSuppressEdgeNavigation` decides whether a touchstart landed on one,
 * and it answers from two constants in `MobileSwipeContainer.tsx`.
 *
 * Nothing else ties the two together. A guard NARROWER than its strip leaves a
 * band where the touch goes unprevented. WebKit's edge recognizer then runs and
 * a standalone iOS PWA navigates away. A guard WIDER than its strip swallows
 * ordinary touches, blocking vertical scroll in that band.
 *
 * Both were px literals until the guards became rem. That drift was at least
 * visible at one root size. Two rem numbers that merely disagree look correct,
 * which is why this reads the stylesheet rather than restating the numbers.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';

import {
  EDGE_NAV_GUARD_LEFT_REM,
  EDGE_NAV_GUARD_RIGHT_REM,
} from '../../components/layout/MobileSwipeContainer';

const here = dirname(fileURLToPath(import.meta.url));
const mobileCss: string = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

/** The `width` of a `.edge-swipe-*` rule, in rem. */
function zoneWidthRem(selector: string): number {
  const rule = new RegExp(`\\.${selector}\\s*\\{([^}]*)\\}`).exec(mobileCss);
  expect(rule, `no .${selector} rule in mobile.css`).not.toBeNull();
  const width = /width:\s*([\d.]+)rem/.exec(rule![1]);
  expect(width, `.${selector} has no rem width: ${rule![1]}`).not.toBeNull();
  return Number(width![1]);
}

describe('the edge-nav guards mirror their CSS strips', () => {
  it('reads a rem width for both strips', () => {
    expect(zoneWidthRem('edge-swipe-left')).toBeGreaterThan(0);
    expect(zoneWidthRem('edge-swipe-right')).toBeGreaterThan(0);
  });

  it('covers the left strip exactly', () => {
    expect(EDGE_NAV_GUARD_LEFT_REM).toBe(zoneWidthRem('edge-swipe-left'));
  });

  it('covers the right strip exactly', () => {
    expect(EDGE_NAV_GUARD_RIGHT_REM).toBe(zoneWidthRem('edge-swipe-right'));
  });
});
