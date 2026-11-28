import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsView: string = readFileSync(resolve(here, '../SettingsView.tsx'), 'utf-8');

describe('Settings → Notifications: the In-app toasts row', () => {
  const toastRow = settingsView.indexOf('data-search-anchor="appearance:in-app-toasts"');
  const deviceError = settingsView.indexOf('<LoadableError noun="devices"');

  it('is on the page at all', () => {
    expect(toastRow).toBeGreaterThan(-1);
    expect(deviceError).toBeGreaterThan(-1);
  });

  it('renders outside the device-list branch, so a failed list cannot hide it', () => {
    const fragmentClose = settingsView.indexOf('</>', deviceError);
    expect(fragmentClose).toBeGreaterThan(-1);
    expect(toastRow).toBeGreaterThan(fragmentClose);
  });

  it('gates its toggle on preferences, not on the device list', () => {
    const toggle = settingsView.slice(toastRow, toastRow + 1200);
    expect(toggle).toMatch(/loaded=\{preferences\.value\.status === 'loaded'\}/);
    expect(toggle).toMatch(/setNotificationToasts/);
  });

  it('puts its explanation on the row label, like every one-control row here', () => {
    expect(settingsView).toMatch(
      /<span class="settings-row-label">\s*In-app toasts\s*<Explainer title="In-app toasts">/,
    );
  });

  it('says the switch reaches every device, since the one above it does not', () => {
    const explainer = settingsView.slice(toastRow, toastRow + 1200);
    expect(explainer).toMatch(/every device/);
  });
});
