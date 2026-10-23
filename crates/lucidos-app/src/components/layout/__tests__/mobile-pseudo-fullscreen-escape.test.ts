import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Locks the pseudo-fullscreen escape. On iOS PWA the app "fullscreen" button
// falls back to CSS pseudo-fullscreen: the app iframe's .app-ui-fullscreen
// overlay is position:fixed and must cover the real viewport. Three swipe
// ancestors each carry a transform (compositor layer / inline pane offset),
// and ANY transformed ancestor forms a containing block that traps a fixed
// descendant — so the overlay gets clipped to the swipe viewport, the thread
// drawer shows through, and the page appears frozen. Every transformed
// ancestor's transform must be reset under :root[data-pseudo-fullscreen].
// Regression: .mobile-swipe-container was originally the missing one (track
// and pane were reset, the container was not) — see the bugfix that added it.

const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss = readFileSync(
  resolve(here, '../../../styles/mobile.css'),
  'utf-8',
);

/** True iff some `:root[data-pseudo-fullscreen] .<className>` rule resets the
 *  transform to none. The selector half tolerates a comma-grouped sibling
 *  before the `{`; the body half matches `transform: none` (also satisfied by
 *  the `-webkit-transform: none` prefix that precedes it). */
function resetsTransform(className: string): boolean {
  const re = new RegExp(
    `:root\\[data-pseudo-fullscreen\\][^{}]*\\.${className}[^{}]*\\{[^}]*transform\\s*:\\s*none`,
  );
  return re.test(mobileCss);
}

describe('pseudo-fullscreen escape: containing-block ancestors reset', () => {
  // All three are the elements that carry a transform in the base mobile
  // layout (container + pane: translateZ(0); track: inline transform +
  // will-change:transform). Each must drop it so position:fixed escapes.
  it.each([
    'mobile-swipe-container',
    'mobile-swipe-track',
    'mobile-swipe-pane',
  ])('resets transform on .%s under data-pseudo-fullscreen', (className) => {
    expect(resetsTransform(className)).toBe(true);
  });
});
