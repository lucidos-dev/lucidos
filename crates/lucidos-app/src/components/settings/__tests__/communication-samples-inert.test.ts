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

const ALLOWED_IMPORTS = [
  '../../store/store',
  '../../store/progressDialogCopy',
  '../shared/Dropdown',
];

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
  return [...source.matchAll(/(^|\n)import\s+(?!type\s)[^;]*?from\s+'([^']+)'/g)].map((m) => m[2]);
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
    expect(page).toContain('useEffect(() => stopSampleProgressDialog, [])');
  });

  it('keeps the banners as the real bodies rather than copies', () => {
    expect(page).toContain('backupReminderBody(');
    expect(page).toContain('connectionBannerBody(');
    expect(page).toContain('ingressBannerBody(');
    expect(page).toContain('waitingPanelBody(');
  });

  it('shows the waiting panel only drilled in, where no row can stop a real wait', () => {
    expect(page).toContain('condition: SAMPLE_CONDITION');
    expect(page).toContain('onBack: () => {}');
  });

  it('gives the ingress bar an inert button, since its real one navigates', () => {
    expect(page).toContain('onOpenWebhooks: () => {}');
  });
});
