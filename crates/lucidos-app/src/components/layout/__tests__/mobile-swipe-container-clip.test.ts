import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Locks compositor-clip rules on .mobile-swipe-container. Without these,
// the inner .mobile-swipe-track (which has will-change:transform and is its
// own compositor layer) can momentarily render outside this container's
// overflow:hidden bounds on iOS Safari during rapid back-and-forth swipes —
// exposing all three full-width panes at once before self-correcting.

const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss = readFileSync(
  resolve(here, '../../../styles/mobile.css'),
  'utf-8',
);

function containerRuleBody(): string {
  const m = mobileCss.match(/(^|\n)\s*\.mobile-swipe-container\s*\{([^}]*)\}/);
  if (!m) throw new Error('.mobile-swipe-container base rule not found');
  return m[2];
}

describe('.mobile-swipe-container compositor clip', () => {
  it('clips overflow', () => {
    expect(containerRuleBody()).toMatch(/overflow\s*:\s*hidden/);
  });

  it('forces its own compositor layer so overflow:hidden clips the will-change track', () => {
    // translateZ / translate3d / will-change:transform / contain:paint each
    // promote to a compositor layer that clips at composition time.
    expect(containerRuleBody()).toMatch(
      /transform\s*:\s*translate(Z|3d)|will-change\s*:[^;]*\btransform\b|contain\s*:[^;]*\bpaint\b/,
    );
  });
});
