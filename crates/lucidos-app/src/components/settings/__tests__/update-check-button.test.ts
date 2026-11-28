import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const PAGE = readFileSync(resolve(here, '..', 'UpdatesPage.tsx'), 'utf8');

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('the update-check button', () => {
  const page = stripComments(PAGE);

  it('reports the in-flight check on its own label', () => {
    expect(page).toMatch(/updateControlLabel\([^)]*,\s*checking\)/);
  });

  it('offers only what this page itself can do', () => {
    expect(page).toContain("canInstallHere ? 'install' : 'check'");
    expect(page).not.toContain("'guide'");
  });

  it('offers a phone an update button only through its desktop app', () => {
    expect(page).toContain('(canInstallHere || canCheckHere) && !thisDeviceIsMobile()');
    expect(page).toContain("route === 'check' && thisDeviceIsMobile()");
  });

  it('leaves the capability itself alone', () => {
    expect(page).toContain('const canCheckHere = canCheckForUpdatesHere();');
  });

  it('keeps a live run outside the capability gate', () => {
    expect(page).toMatch(/\{updateNarration\s*\n\s*\?/);
    expect(page).toContain('{(updateNarration || offersUpdateControl) && (');
  });

  it('refuses a second check while one is running', () => {
    expect(page).toMatch(/disabled=\{checking\}/);
  });

  it('takes the in-flight state from the signal the action owns', () => {
    expect(page).toContain('appUpdateCheckInFlight.value');
  });

  it('reports the verdict through the one shared action', () => {
    expect(page).toContain('followUpdateRoute(pageRoute)');
    expect(page).not.toContain('checkForUpdatesNow');
  });

  it('never re-reads the signals to decide what the check found', () => {
    expect(page).not.toContain("showToast('Lucidos is up to date'");
    expect(page).not.toMatch(/if\s*\(appUpdateCheckError\.value\)\s*\{/);
  });
});
