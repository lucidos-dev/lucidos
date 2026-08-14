/**
 * The surface gallery's one hard promise: every button renders a surface and
 * stops. No restart, no download, no navigation, no preference write.
 *
 * A source scan rather than a behavioural test. The failure it guards is
 * someone reaching for the real action to make a sample "more realistic". That
 * reads as an improvement in review, and it turns a preview page into a way to
 * restart the engine by accident. The import list is the choke point: a sample
 * cannot call what the module never imported.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const samples = readFileSync(resolve(here, '../communicationSamples.ts'), 'utf-8');
const page = readFileSync(resolve(here, '../CommunicationSurfacesPage.tsx'), 'utf-8');

/** Every module the samples are allowed to reach. Anything else is either an
 *  action or a route, and neither belongs on a preview page. */
const ALLOWED_IMPORTS = ['../../store/store', '../shared/Dropdown'];

/** Names that perform the operations the gallery only depicts. Matched as whole
 *  words so a sample's own `sampleRestartDialog` is not read as a restart. */
const FORBIDDEN = [
  'restartEngine',
  'confirmAndRestartEngine',
  'installAppUpdate',
  'cancelAppUpdate',
  'triggerRebuild',
  'switchToNewVersion',
  'savePreference',
  'openSettingsSubview',
  'handleNavigationRequest',
  'focusThread',
  'invoke',
  'fetch',
];

function importedModules(source: string): string[] {
  return [...source.matchAll(/from\s+'([^']+)'/g)].map((m) => m[1]);
}

describe('the communication-surface gallery is inert', () => {
  it('imports nothing but the store and the dropdown', () => {
    for (const mod of importedModules(samples)) {
      expect(ALLOWED_IMPORTS, `communicationSamples.ts imports ${mod}`).toContain(mod);
    }
  });

  it('names no action that would perform a real operation', () => {
    for (const name of FORBIDDEN) {
      expect(new RegExp(`\\b${name}\\s*\\(`).test(samples), `${name}( in samples`).toBe(false);
      expect(new RegExp(`\\b${name}\\s*\\(`).test(page), `${name}( on the page`).toBe(false);
    }
  });

  it('clears the progress slot when the page unmounts', () => {
    // A fake run is an interval writing a store signal. A page that forgot this
    // would leave a modal over the app with nothing left to stop it.
    expect(page).toContain('useEffect(() => stopSampleProgressDialog, [])');
  });

  it('keeps the banners as the real bodies rather than copies', () => {
    // Calling the shipped pure bodies is what stops the gallery drifting from
    // what the app actually renders.
    expect(page).toContain('backupReminderBody(');
    expect(page).toContain('connectionBannerBody(');
  });
});
