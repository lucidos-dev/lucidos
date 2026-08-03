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
 *  the exact shape this guard has to catch.
 *
 *  A `//` preceded by a backslash is left alone: that is the tail of a regex
 *  literal such as `/^https?:\/\//`, whose escaped slash and closing delimiter
 *  read as a line comment and would otherwise swallow the rest of the line
 *  (taking the scheme test the external-link guard asserts on with it). */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\])\/\/.*$/gm, '$1');
}

/** The body of a `function <name>(…)` declaration, by brace matching from its
 *  opening `{`. Comments are stripped first, so a brace inside one cannot skew
 *  the match. */
function handlerBody(src: string, declaration: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf(declaration);
  expect(start, `useStartup.ts must declare \`${declaration}\``).toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error(`unbalanced braces in \`${declaration}\`, so the guard cannot bound the handler`);
}

/** Every update surface that MUST be reconciled when the page comes back to the
 *  foreground, and what silently rots if its call goes missing. */
const RESUME_RECONCILED: Array<[call: string, whatBreaks: string]> = [
  ['reg?.update()', 'a new frontend build is never picked up by the service worker'],
  ['syncClientUpdateFromBuild(', 'the client-update badge misses a build that landed while away'],
  ['checkEngineVersion(', 'an engine build that finished while away shows as still spinning'],
  ['recheckAppUpdateOnResume(', 'a packaged release goes unannounced until the next poll'],
  ['flushPendingPreferenceWrites(', 'a settings change WebKit aborted at suspend never reaches the engine, so the device and the server disagree until the next reload'],
];

describe('useStartup resume reconciliation', () => {
  const body = handlerBody(readFileSync(SOURCE, 'utf8'), 'function onResume()');

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

/**
 * The delegated anchor route in `onGlobalClick` is the ONLY thing standing
 * between a plain `<a href="https://…">` and the browser's own navigation.
 * Chat markdown (`renderMarkdown` / `linkifyPaths`), the rendered markdown file
 * preview and the settings rows all emit raw `target="_blank"` anchors with no
 * component handler of their own, so this handler is what funnels every one of
 * them into `openUrl`.
 *
 * That funnel is load-bearing beyond tidiness: on an installed iOS PWA a
 * `target="_blank"` anchor opens the inescapable in-app web view, and `openUrl`
 * is where the `x-safari-` hand-off lives (`utils/openExternalUrl.ts`). Delete
 * the branch and the platform fix silently stops reaching the surfaces the user
 * actually taps. Same source-scan reasoning as the resume guard above.
 */
describe('useStartup external-link delegation', () => {
  const body = handlerBody(readFileSync(SOURCE, 'utf8'), 'function onGlobalClick(');

  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startAppUpdateChecks(');
    expect(body.length).toBeGreaterThan(0);
  });

  it('claims every http(s) anchor and routes it through openUrl', () => {
    expect(body).toContain(`closest('a[href]')`);
    expect(body).toContain('/^https?:\\/\\//.test(href)');
    expect(body).toContain('preventDefault()');
    expect(body).toContain('openUrl(href)');
  });

  it('never opens an external URL itself, so the platform routing has one home', () => {
    expect(body).not.toContain('window.open(');
  });
});
