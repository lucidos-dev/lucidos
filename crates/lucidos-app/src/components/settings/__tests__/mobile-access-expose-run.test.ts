/**
 * The Expose button reads the run from the STORE, never from a page-local flag.
 *
 * An Expose run can legitimately spend minutes waiting for a tailnet approval,
 * and it narrates itself on the brand badge and the shared status toast from
 * anywhere in the app. A `useState` flag on this page would be lost the moment
 * the user navigated away and came back, leaving a live run behind an "Expose"
 * button that would start a second one. (Rust refuses that second run, but a
 * button offering something that will be refused is still the wrong button.)
 *
 * Source-scan rather than a mounted render, for the reason
 * `mobile-access-row-reachable.test.ts` gives: `SettingsView` pulls in the whole
 * store, the model registry, OAuth and device state. The behaviour of the run
 * itself is covered by `store/actions/backgroundActivity.test.ts`.
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

describe('the Expose button and the shared run', () => {
  const page = stripComments(PAGE);

  it('takes its state from the store signal', () => {
    expect(page).toMatch(/const serveRunning = tailscaleServeRun\.value !== null/);
    expect(page).toMatch(/disabled=\{serveRunning\}/);
  });

  /** The flag that used to hold it. `busy` survives for Sign in, which is a
   *  single interactive call with no progress surface of its own. */
  it('keeps no page-local flag for the run', () => {
    expect(page).not.toMatch(/busy === 'serve'/);
    expect(page).not.toMatch(/setBusy\('serve'\)/);
  });

  it('announces the run on the click, and refuses a second one', () => {
    expect(page).toMatch(/beginTailscaleServeRun\(\)/);
    // The click guard, so a double-press cannot replace a live run's first
    // frame with a fresh `starting` one.
    expect(page).toMatch(/if \(tailscaleServeRun\.value\) return;/);
  });

  /** The outcome comes from the terminal progress frame, which knows whether
   *  the run succeeded, was cancelled, or failed. A success toast fired from
   *  the promise as well would double up on it. */
  it('leaves the outcome to the progress frame', () => {
    expect(page).not.toMatch(/Engine exposed at/);
  });

  /** Both halves of "no frame ever arrived", which is what a failed progress
   *  subscription looks like. Without them a run that already finished would
   *  leave the badge spinning and Expose disabled for the rest of the session. */
  it('settles the run itself when no terminal frame arrived', () => {
    // Resolved: settle from the URL the command returned.
    expect(page).toMatch(/applyTailscaleServeProgress\(\{ phase: 'done', url \}\)/);
    // Rejected: clear, and report the error the frame would have carried.
    expect(page).toMatch(/clearTailscaleServeRun\(\)/);
    // Both guarded on the run still being set, so a frame that DID arrive is
    // never narrated twice.
    expect(page.match(/if \(tailscaleServeRun\.value\) \{/g) ?? []).toHaveLength(2);
  });
});
