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

describe('Mobile Access failure toasts', () => {
  const page = stripComments(PAGE);

  it('passes the error through untouched', () => {
    const passthrough = page.match(/showToast\(errorDetail\(e\), 'error'\)/g) ?? [];
    expect(passthrough).toHaveLength(2);
  });

  it('never re-frames the error with an action prefix', () => {
    expect(page).not.toMatch(/Tailscale serve failed/);
    expect(page).not.toMatch(/Tailscale sign-in failed/);
  });
});
