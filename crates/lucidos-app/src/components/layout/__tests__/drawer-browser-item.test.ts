/**
 * The menu drawer's Browser row is the only entry point to the experimental
 * in-app browser, so it must be gated on `inAppBrowserAvailable()`: the desktop
 * app AND the Settings > Experimental opt-in. Gating it on `isTauri()` alone
 * shipped the row with the toggle off, where `openUrl` deliberately routes to
 * the OS opener, so a row labelled "Browser" just launched the system browser on
 * google.com.
 *
 * A source scan rather than a render: the row exists only under Tauri, which no
 * Playwright project can be (`isTauri()` reads a webview-injected global), so
 * the gate is unreachable from browser e2e. The predicate's own behaviour is
 * covered in `store/actions/preferences.test.ts`; what this pins is that the row
 * hangs off it.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const drawerSource = readFileSync(
  fileURLToPath(new URL('../Drawer.tsx', import.meta.url)),
  'utf8',
);

/** Every `{<expr> && (` JSX gate opened before `index`, innermost last. */
function gatesBefore(index: number): string[] {
  return [...drawerSource.slice(0, index).matchAll(/\{([^{}\n]+?) && \(/g)].map((m) => m[1]);
}

describe('the menu drawer Browser row', () => {
  it('is gated on inAppBrowserAvailable(), never on the platform check alone', () => {
    // The row's own label, on its own line: the prose mentions of "Browser" in
    // the surrounding comments are mid-sentence and never match.
    const label = drawerSource.match(/^[ \t]*Browser$/m);
    expect(label?.index, 'the Browser row label').toBeDefined();

    const gates = gatesBefore(label!.index!);
    expect(gates[gates.length - 1]).toBe('inAppBrowserAvailable()');
  });
});
