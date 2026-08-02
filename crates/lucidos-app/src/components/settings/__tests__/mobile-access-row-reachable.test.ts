/**
 * Mobile Access must be reachable from the device it is written for.
 *
 * The bug this pins is the same shape as `external-links-row-reachable.test.ts`,
 * and is why that file's reasoning is reused here. The nav entry was filtered to
 * `isTauri() && enginePackaged.value`, so the page existed only inside the
 * packaged desktop app. Its whole purpose is getting the user onto their phone,
 * and its install half (what Tailscale buys you, Get Tailscale, the phone steps)
 * needs no Tauri IPC at all. Nothing failed: on a phone the row simply was not
 * there.
 *
 * The machine-side half genuinely is desktop-only (`get_connect_info`,
 * `tailscale_up`, `tailscale_serve` are Tauri commands with no engine HTTP
 * equivalent), so the page splits rather than moves. This guard therefore checks
 * BOTH halves of that split, since widening the nav without gating the controls
 * would put buttons on a phone that throw the moment they are pressed.
 *
 * Source-scan rather than a mounted render, for the reason the sibling file
 * gives: `SettingsView` pulls in the whole store, the model registry, OAuth and
 * device state.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SETTINGS_VIEW = readFileSync(resolve(here, '..', 'SettingsView.tsx'), 'utf8');
const PAGE = readFileSync(resolve(here, '..', 'MobileAccessPage.tsx'), 'utf8');

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Mobile Access reachability', () => {
  const nav = stripComments(SETTINGS_VIEW);
  const page = stripComments(PAGE);

  it('does not gate its nav entry on the desktop app', () => {
    // Any `key === 'mobile-access'` branch in the nav filter is a regression:
    // the row must list everywhere, because a phone is a first-class reader.
    expect(nav).not.toMatch(/key === 'mobile-access'/);
  });

  it('still lists the entry, rather than having quietly lost it', () => {
    // The opposite failure of the one above: deleting the filter branch AND the
    // entry would also make the assertion above pass.
    expect(nav).toContain(`case 'mobile-access': return <MobileAccessPage />;`);
  });

  it('gates the machine-side half inside the page instead', () => {
    // Those controls are Tauri commands; the bridge dereferences
    // `window.__TAURI_INTERNALS__!` synchronously, so a button rendered in a
    // browser throws rather than rejecting.
    expect(page).toContain('const showMachineHalf = isTauri() && enginePackaged.value;');
    expect(page).toContain('{showMachineHalf && connectUrlsSection()}');
  });

  it('skips the fetch off the desktop app rather than failing the pane', () => {
    // `toFailed(...)` here would render the error card INSTEAD of the whole
    // page, blanking the install half that a phone came for. There is nothing
    // to fetch off Tauri, so there is no failure to report.
    expect(page).toContain('if (!showMachineHalf) return;');
    expect(page).not.toContain('Mobile access is only available in the desktop app.');
  });

  it('shows the install half regardless of platform', () => {
    // The ungated branch: the Tailscale intro and the phone steps render either
    // way, with the install row addressed to whichever device is reading.
    expect(page).toContain('<InstallTailscaleRow onPhone={true} />');
    expect(page).toContain('<InstallTailscaleRow onPhone={false} />');
  });
});
