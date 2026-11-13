/** Every icon-only retry control draws the one shared `RetryIcon`, and that
 *  glyph's arc runs into its arrowhead. A retry button once wore a play icon,
 *  and the glyph's head once floated off its arc. */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, relative } from 'node:path';

const SRC: string = join(dirname(fileURLToPath(import.meta.url)), '../../..');

function tsxFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry: { name: string; isDirectory(): boolean }) => {
    const path: string = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === '__tests__' ? [] : tsxFiles(path);
    return entry.name.endsWith('.tsx') ? [path] : [];
  });
}

/** A control whose accessible name or tooltip says Retry. */
const RETRY_LABEL = /(?:aria-label|data-tooltip)=\{?[^>\n]*Retry/;

describe('one retry glyph', () => {
  it('draws the arc into the arrowhead corner', () => {
    const icons = readFileSync(join(SRC, 'components/shared/icons.tsx'), 'utf8');
    const body = icons.slice(icons.indexOf('export function RetryIcon'));
    expect(body).toContain('d="M3 12A9 9 0 1 0 5.64 5.64L3 8"');
    expect(body).toContain('d="M3 3v5h5"');
  });

  it('is declared once, in the shared icons', () => {
    const declaring = tsxFiles(SRC).filter((f) => /function RetryIcon\b/.test(readFileSync(f, 'utf8')));
    expect(declaring.map((f) => relative(SRC, f))).toEqual(['components/shared/icons.tsx']);
  });

  it('is what every icon-only retry control renders', () => {
    const offenders = tsxFiles(SRC)
      .filter((f) => {
        const src = readFileSync(f, 'utf8');
        return RETRY_LABEL.test(src) && !src.includes('<RetryIcon');
      })
      .map((f) => relative(SRC, f));
    expect(offenders).toEqual([]);
  });
});
