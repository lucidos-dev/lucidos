/**
 * Settings, System: the Check for Updates button reports its own check, and
 * says exactly what that check returned.
 *
 * Two defects on a `.dmg` install produced this test. The button gave no sign
 * it was doing anything, so it read as dead across a network round-trip. And
 * the handler awaited a check, then decided "up to date" by re-reading
 * `packagedUpdateVersion()`, a signal the BACKGROUND poll also writes. A click
 * could therefore report another request's state, which showed "Lucidos is up
 * to date" a moment before the offer toast for 0.29.0.
 *
 * Source-scan rather than a mounted render, for the reason
 * `mobile-access-error-toast.test.ts` gives: the page pulls in the whole store.
 * The wording and the label are unit-tested where they are pure, in
 * `store/actions/app-update.test.ts`.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const PAGE = readFileSync(resolve(here, '..', 'SystemPage.tsx'), 'utf8');

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('the update-check button', () => {
  const page = stripComments(PAGE);

  it('reports the in-flight check on its own label', () => {
    expect(page).toMatch(/updateControlLabel\([^)]*,\s*checking\)/);
  });

  // This page is where the `guide` route SENDS people, so its own button must
  // never wear that route: it would point back here and go nowhere.
  it('offers only what this page itself can do', () => {
    expect(page).toContain("canInstallHere ? 'install' : 'check'");
    expect(page).not.toContain("'guide'");
  });

  // A live run renders on its own, ahead of the capability gate. That gate
  // reads signals a background poll also writes. Folding the two together
  // would let a mid-run refresh take Cancel away from a live download.
  it('keeps a live run outside the capability gate', () => {
    expect(page).toMatch(/\{updateNarration\s*\n\s*\?/);
    expect(page).toMatch(/:\s*\(canInstallHere \|\| canCheckHere\) &&/);
  });

  it('refuses a second check while one is running', () => {
    expect(page).toMatch(/disabled=\{checking\}/);
  });

  it('takes the in-flight state from the signal the action owns', () => {
    expect(page).toContain('appUpdateCheckInFlight.value');
  });

  // The verdict-reporting itself moved into `followUpdateRoute`, where
  // `store/actions/app-update.test.ts` pins it. What this page owes is the
  // delegation: it must not re-implement the branch and drift from it.
  it('reports the verdict through the one shared action', () => {
    expect(page).toContain('followUpdateRoute(pageRoute)');
    expect(page).not.toContain('checkForUpdatesNow');
  });

  // The regression itself. Both reads happened AFTER the await, against signals
  // the background poll writes too.
  it('never re-reads the signals to decide what the check found', () => {
    expect(page).not.toContain("showToast('Lucidos is up to date'");
    expect(page).not.toMatch(/if\s*\(appUpdateCheckError\.value\)\s*\{/);
  });
});
