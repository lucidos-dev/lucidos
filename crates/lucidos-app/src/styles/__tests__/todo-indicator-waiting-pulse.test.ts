/**
 * The todo indicator's waiting state is the app's gray pulse, not a colour.
 *
 * It used to be `--accent-yellow`, on the stated grounds that it matched the
 * waiting indicator beside it. It matched nothing: yellow is the CAUTION tone
 * in this palette, and a thread parked on an event wait is not a caution. Nor
 * can this button borrow that neighbour's accent, since accent is the hue it
 * already spends on in-progress. The app already has a waiting language:
 * `.progress-dot-waiting` is a plain `--text-secondary` dot pulsing on
 * `progress-pulse`, and the indicator now speaks it.
 *
 * Two things nothing else in the gate can see, which is why this scan exists:
 *
 * 1. The keyframes live in another stylesheet (`chat/input-messages.css`).
 *    Keyframes are global, so the reference works, but rename or move them out
 *    of the bundle and the indicator silently stops moving. `tsc` never reads
 *    CSS and `vite build` only fails on syntax, so an unresolvable animation
 *    name builds perfectly clean.
 * 2. The state carries no `color`, which is what makes it the SAME gray as idle
 *    with motion as the only difference. A colour added back here would read as
 *    a tidy-up rather than as the regression it is.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
/** `crates/lucidos-app/src/styles/`, from `src/styles/__tests__/`. */
const STYLES = resolve(here, '..');
const SRC = resolve(here, '../..');

const ANIMATION = 'progress-pulse';
const WAITING = '[data-role="todo-indicator"][data-state="waiting"]';

/** A stylesheet with its comments stripped. Load-bearing for both scans below:
 *  `global/shared-components.css` documents its own import in prose, so a raw
 *  read walks off to a path that does not exist, and a keyframes block quoted
 *  in a comment would count as declared. */
function readCss(file: string): string {
  return readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
}

/** Every stylesheet the host bundle loads: the `./styles/*.css` entries
 *  `main.tsx` imports, plus everything those `@import`, transitively. Walked
 *  rather than listed so the keyframes count as declared wherever they end up,
 *  and DON'T count if they land in a sheet the bundle never pulls in. */
function bundledStylesheets(): string[] {
  const main = readFileSync(join(SRC, 'main.tsx'), 'utf8');
  const queue = [...main.matchAll(/^import\s+'\.\/styles\/([^']+\.css)';/gm)]
    .map((m) => join(STYLES, m[1]));
  const seen: string[] = [];
  while (queue.length) {
    const file = queue.pop();
    if (seen.includes(file)) continue;
    seen.push(file);
    for (const m of readCss(file).matchAll(/@import\s+'([^']+)'/g)) {
      queue.push(resolve(dirname(file), m[1]));
    }
  }
  return seen;
}

const todoCss = readFileSync(join(STYLES, 'chat', 'todo-list.css'), 'utf8');
const waitingRules = cssRules(todoCss).filter((r) => r.selector.includes(WAITING));

describe('todo indicator: the waiting state', () => {
  it('pulses on the same keyframes as a waiting progress dot', () => {
    const animated = waitingRules.filter((r) => r.props.get('animation')?.includes(ANIMATION));
    expect(animated.length, `exactly one rule for ${WAITING} must animate ${ANIMATION}`).toBe(1);
  });

  it('animates the glyph, not the button', () => {
    // On the button the opacity pulse would take the hover background and the
    // focus ring with it, so a keyboard user's focus ring would fade in and out.
    const animated = waitingRules.find((r) => r.props.get('animation')?.includes(ANIMATION));
    expect(animated?.selector ?? '').toMatch(/\bsvg$/);
  });

  it('paints no colour of its own, so waiting is the idle gray and motion is the difference', () => {
    for (const rule of waitingRules) {
      expect(
        rule.props.get('color'),
        `${rule.selector} must not tint the waiting state`,
      ).toBeUndefined();
    }
  });

  it('references keyframes the bundle actually declares', () => {
    const declaring = bundledStylesheets().filter((f: string) =>
      new RegExp(`@keyframes\\s+${ANIMATION}\\b`).test(readCss(f)),
    );
    expect(
      declaring.length,
      `exactly one bundled stylesheet must declare @keyframes ${ANIMATION}`,
    ).toBe(1);
  });
});
