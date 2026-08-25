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
    expect(page).toContain('updateControlLabel(checking');
  });

  it('refuses a second check while one is running', () => {
    expect(page).toMatch(/onClick=\{handleAppUpdate\}\s+disabled=\{checking\}/);
  });

  it('takes the in-flight state from the signal the action owns', () => {
    expect(page).toContain('appUpdateCheckInFlight.value');
  });

  it('reports the verdict the check returned', () => {
    expect(page).toContain('reportUpdateCheck(await checkForUpdatesNow())');
  });

  // The regression itself. Both reads happened AFTER the await, against signals
  // the background poll writes too.
  it('never re-reads the signals to decide what the check found', () => {
    expect(page).not.toContain("showToast('Lucidos is up to date'");
    expect(page).not.toMatch(/if\s*\(appUpdateCheckError\.value\)\s*\{/);
  });
});
