import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Locks visual-isolation rules on .mobile-swipe-pane. Without these, adjacent
// panes (and especially their iframes on iOS Safari) bleed through the active
// pane during a swipe.

const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss = readFileSync(
  resolve(here, '../../../styles/mobile.css'),
  'utf-8',
);

/** Body of the base `.mobile-swipe-pane { ... }` rule (excludes descendant selectors). */
function paneRuleBody(): string {
  // Anchor on a line that starts (after indent) with `.mobile-swipe-pane {`
  // — descendant selectors like `.mobile-swipe-pane .foo {` won't match because
  // the brace is preceded by something other than the class name.
  const m = mobileCss.match(/(^|\n)\s*\.mobile-swipe-pane\s*\{([^}]*)\}/);
  if (!m) throw new Error('.mobile-swipe-pane base rule not found');
  return m[2];
}

describe('.mobile-swipe-pane visual isolation', () => {
  it('has an opaque background so adjacent panes cannot show through', () => {
    expect(paneRuleBody()).toMatch(/(^|\s|;)background(-color)?\s*:/);
  });

  it('establishes its own stacking context to contain z-indexes', () => {
    // `isolation: isolate`, `contain: paint`, or `transform: translateZ(...)`
    // each establish a stacking context.
    expect(paneRuleBody()).toMatch(
      /isolation\s*:\s*isolate|contain\s*:[^;]*\bpaint\b|transform\s*:\s*translateZ/,
    );
  });

  it('forces its own compositor layer so iframe descendants clip during swipes', () => {
    // `isolation: isolate` creates a stacking context but NOT a compositor
    // layer. On iOS Safari, iframe descendants need an ancestor compositor
    // layer to clip during transforms — without it, the iframe paints
    // outside its pane during a swipe. `will-change: transform` is only a
    // hint and doesn't guarantee promotion in WebKit, so it's not enough.
    expect(paneRuleBody()).toMatch(
      /transform\s*:\s*translate(Z|3d)|contain\s*:[^;]*\bpaint\b/,
    );
  });
});
