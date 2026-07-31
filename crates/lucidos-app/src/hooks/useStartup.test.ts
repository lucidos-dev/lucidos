/**
 * Regression guard for the resume-reconciliation set.
 *
 * `useStartup`'s `onResume` is what a long-resident client relies on to notice
 * anything that changed while the user was away: it fires on window `focus`,
 * `visibilitychange` and `pageshow`. Four separate update surfaces reconcile
 * there, and each one is a single unremarkable call that a refactor can drop
 * without breaking a type or a test. Losing one is invisible until a user asks
 * why they were never told about a release.
 *
 * That is not hypothetical. The packaged app updater was missing from this set
 * until 2026-07-31, so a 0.18.0 client that neither remounted nor waited out its
 * poll interval reported itself current for a whole morning with 0.18.2 already
 * published (`store/actions/app-update.ts`). This test exists so the set can only
 * shrink deliberately.
 *
 * A source scan rather than a mounted-hook test: `useStartup` wires SSE, service
 * workers, timers, presence and push, so standing it up in jsdom to observe one
 * call would cost far more than the invariant is worth, and would pin the
 * mechanism instead of the requirement.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(here, 'useStartup.ts');

/** Strip `//` and block comments so a surviving comment can never stand in for
 *  a deleted call. Dropping the call and leaving the prose that explains it is
 *  the exact shape this guard has to catch. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
}

/** The body of `function onResume()`, by brace matching from its opening `{`.
 *  Comments are stripped first, so a brace inside one cannot skew the match. */
function resumeHandlerBody(src: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf('function onResume()');
  expect(start, 'useStartup.ts must declare `function onResume()`').toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error('unbalanced braces in `onResume`, so the guard cannot bound the handler');
}

/** Every update surface that MUST be reconciled when the page comes back to the
 *  foreground, and what silently rots if its call goes missing. */
const RESUME_RECONCILED: Array<[call: string, whatBreaks: string]> = [
  ['reg?.update()', 'a new frontend build is never picked up by the service worker'],
  ['syncClientUpdateFromBuild(', 'the client-update badge misses a build that landed while away'],
  ['checkEngineVersion(', 'an engine build that finished while away shows as still spinning'],
  ['recheckAppUpdateOnResume(', 'a packaged release goes unannounced until the next poll'],
];

describe('useStartup resume reconciliation', () => {
  const body = resumeHandlerBody(readFileSync(SOURCE, 'utf8'));

  // Proves the brace match actually bounded the handler. `startAppUpdateChecks`
  // is called once in the effect body and never on resume, so seeing it here
  // would mean the slice overran and every assertion below had become vacuous.
  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startAppUpdateChecks(');
    expect(body.length).toBeGreaterThan(0);
  });

  it.each(RESUME_RECONCILED)('reconciles %s on resume', (call, whatBreaks) => {
    expect(body, `dropping this call means ${whatBreaks}`).toContain(call);
  });
});
