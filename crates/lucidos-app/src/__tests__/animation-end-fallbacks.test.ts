/**
 * No UI may wait on an animation end event that might never come.
 *
 * An element with no animation fires no end event, and reduced motion removes
 * animations. A UI state that clears only on that event then never clears, and
 * an overlay left open inerts the whole shell. The in-app Motion setting makes
 * that path common.
 *
 * So every listener is audited here, with how it completes without its event.
 * A new listener fails this test until it is added, which is the point: the
 * author has to answer the question before it ships.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
import { clientSourcePaths } from '../styles/__tests__/css-rule-helpers';

const srcRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** How each listener completes without its event. `requires` must appear in
 *  the file's code, so the answer cannot rot into a claim. */
const AUDITED: Record<string, { how: string; requires: RegExp | null }> = {
  'components/layout/Drawer.tsx': {
    how: 'closes at once under reduced motion, and a scaled fallback timer closes a stalled slide-out',
    requires: /isReducedMotion\(\)[\s\S]*setTimeout\(/,
  },
  'components/layout/ThreadPane.tsx': {
    how: 'skips the FLIP under reduced motion, and a scaled safety timer clears the gate',
    requires: /setTimeout\(clearAnimation/,
  },
  'components/chat/promptResize.ts': {
    how: 'a scaled safety timer and a hidden-page check settle the height ease',
    requires: /setTimeout\(finish/,
  },
  'components/shared/ImagePopup.tsx': {
    how: 'a scaled safety timer runs the swipe cleanup',
    requires: /setTimeout\(finish/,
  },
  'utils/bootSplash.ts': {
    how: 'a scaled fallback timer removes the splash',
    requires: /setTimeout\(remove/,
  },
  'components/layout/MobileSwipeContainer.tsx': {
    how: 'a reconciler only: it corrects a drifted transform and gates nothing',
    requires: null,
  },
};

/** Code with comments removed, so a doc comment naming an event is not a use. */
function code(path: string): string {
  return readFileSync(path, 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/.*$/gm, '$1');
}

const END_EVENT = /['"](?:animationend|transitionend)['"]|\bon(?:Animation|Transition)End\b/;

describe('animation end listeners', () => {
  const listeners = clientSourcePaths(srcRoot)
    .filter((path) => END_EVENT.test(code(path)))
    .map((path) => relative(srcRoot, path))
    .sort();

  it('are all audited', () => {
    expect(
      listeners,
      'A new end-event listener must complete without its event. Add it to AUDITED '
      + 'with a timer fallback or a reduced-motion bypass that reads utils/motion.ts.',
    ).toEqual(Object.keys(AUDITED).sort());
  });

  for (const [file, { how, requires }] of Object.entries(AUDITED)) {
    if (!requires) continue;
    it(`${file}: ${how}`, () => {
      expect(code(resolve(srcRoot, file))).toMatch(requires);
    });
  }
});
