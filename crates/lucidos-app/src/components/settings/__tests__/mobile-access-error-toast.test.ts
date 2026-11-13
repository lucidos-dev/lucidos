/**
 * A failed Sign in / Expose shows the Tauri error verbatim.
 *
 * Every error `mobile.rs` hands back already names what failed: the missing
 * CLI, the missing tailnet address or MagicDNS name, `run_checked`'s
 * "tailscale <cmd> failed: <stderr>", and both "reported success but ..."
 * post-conditions. Wrapping those in an action prefix here produced
 * "Tailscale serve failed: tailscale serve failed: Error: the CLI for serve and
 * funnel has changed", which pushed the CLI's own advice down the toast. That
 * advice is the entire payload when a CLI syntax change is the cause, so the
 * prefix cost the user the one line that named the fix.
 *
 * Source-scan rather than a mounted render, for the reason
 * `mobile-access-row-reachable.test.ts` gives: `SettingsView` pulls in the whole
 * store, the model registry, OAuth and device state.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const PAGE = readFileSync(resolve(here, '..', 'MobileAccessPage.tsx'), 'utf8');

/** Strip comments so the prose explaining a rule can never stand in for it. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('Mobile Access failure toasts', () => {
  const page = stripComments(PAGE);

  it('passes the error through untouched', () => {
    const passthrough = page.match(/showToast\(errorDetail\(e\), 'error'\)/g) ?? [];
    // Both of them: the Sign in handler and the Expose handler. Asserting the
    // count stops one being re-wrapped while the other keeps this test green.
    expect(passthrough).toHaveLength(2);
  });

  it('never re-frames the error with an action prefix', () => {
    // The regression itself, in the two wordings it shipped as. Naming them
    // beats a general "no template literal around errorDetail(e)" scan, which
    // also catches `Couldn't open ${url}: ...` in `openTailscaleDownload`.
    // There the prefix carries the URL, which the opener's own error does not,
    // so it is context rather than duplication.
    expect(page).not.toMatch(/Tailscale serve failed/);
    expect(page).not.toMatch(/Tailscale sign-in failed/);
  });
});
