import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const PAGE = readFileSync(resolve(here, '..', 'MobileAccessPage.tsx'), 'utf8');

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\:])\/\/.*$/gm, '$1');
}

describe('the Expose button and the shared run', () => {
  const page = stripComments(PAGE);

  it('takes its state from the store signal', () => {
    expect(page).toMatch(/const serveRunning = tailscaleServeRun\.value !== null/);
    expect(page).toMatch(/disabled=\{serveRunning\}/);
  });

  it('keeps no page-local flag for the run', () => {
    expect(page).not.toMatch(/busy === 'serve'/);
    expect(page).not.toMatch(/setBusy\('serve'\)/);
  });

  it('announces the run on the click, and refuses a second one', () => {
    expect(page).toMatch(/beginTailscaleServeRun\(\)/);
    expect(page).toMatch(/if \(tailscaleServeRun\.value\) return;/);
  });

  it('leaves the outcome to the progress frame', () => {
    expect(page).not.toMatch(/Engine exposed at/);
  });

  it('settles the run itself when no terminal frame arrived', () => {
    expect(page).toMatch(/applyTailscaleServeProgress\(\{ phase: 'done', url \}\)/);
    expect(page).toMatch(/clearTailscaleServeRun\(\)/);
    expect(page.match(/if \(tailscaleServeRun\.value\) \{/g) ?? []).toHaveLength(2);
  });
});
