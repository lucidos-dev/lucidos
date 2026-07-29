import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../Overlay.tsx'), 'utf-8');

// The whole point of <Overlay> is that the click-outside-dismiss contract is
// centralized so no individual overlay can drop or mis-wire it (the
// SearchEverywhere `anchor=null` / touch-reopen bug). These are tripwires: if a
// future edit guts the contract, one of these fails loudly. The behavior itself
// is exercised end-to-end by `e2e/search-everywhere-close-mobile.spec.ts` (the
// migrated reference overlay).
describe('Overlay — centralizes the dismiss contract', () => {
  it('delegates outside-click dismiss + swallow to useDismissOnOutside, forwarding the anchor', () => {
    // Must call the canonical hook (not a hand-rolled listener) and pass the
    // anchor through — never a hardcoded null, which would treat a toggle button
    // as "outside" and reopen it on touch.
    expect(source).toMatch(/useDismissOnOutside\(\s*open\s*,\s*panelRef\s*,\s*anchor\s*,\s*onClose\s*\)/);
  });

  it('routes Escape through the central overlay stack, not a per-instance keydown listener', () => {
    expect(source).toMatch(/pushOverlay\(/);
    expect(source).toMatch(/removeOverlay\(/);
  });

  it('never hand-rolls its own document dismiss listener', () => {
    // A bare addEventListener('pointerdown'|'mousedown'|'click', …) is exactly
    // the anti-pattern the central component exists to eliminate.
    expect(source).not.toMatch(/addEventListener\(\s*['"](?:pointerdown|mousedown|click)['"]/);
  });
});
