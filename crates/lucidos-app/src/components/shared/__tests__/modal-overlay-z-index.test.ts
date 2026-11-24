import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8');

describe('modal overlay z-index (regression: header punch-through)', () => {
  function tokenValue(name: string): number {
    const m = css.match(new RegExp(`--${name}:\\s*(\\d+)\\s*;`));
    expect(m, `token --${name} not found in global.css :root`).not.toBeNull();
    return parseInt(m![1], 10);
  }

  it('--z-modal must sit strictly above --z-control-panel so the dim backdrop covers the header', () => {
    expect(tokenValue('z-modal')).toBeGreaterThan(tokenValue('z-control-panel'));
  });

  it('--z-toast must stay strictly above --z-modal so toasts remain visible over open modals', () => {
    expect(tokenValue('z-toast')).toBeGreaterThan(tokenValue('z-modal'));
  });
});

describe('no backdrop overlayClass declares its own z-index', () => {
  const SRC: string = resolve(here, '../../..');

  const tsxFiles: string[] = readdirSync(SRC, { recursive: true, encoding: 'utf-8' })
    .filter((f: string) => f.endsWith('.tsx'));

  function overlayClasses(): string[] {
    const found = new Set<string>();
    for (const f of tsxFiles) {
      const text: string = readFileSync(resolve(SRC, f), 'utf-8');
      for (const m of text.matchAll(/overlayClass="([\w-]+)"/g)) found.add(m[1]);
    }
    return [...found];
  }

  it('finds the overlay classes to check', () => {
    expect(overlayClasses().length).toBeGreaterThan(0);
  });

  it('refuses a computed overlayClass, which it cannot follow', () => {
    const computed: string[] = tsxFiles.filter((f: string) =>
      /overlayClass=\{/.test(readFileSync(resolve(SRC, f), 'utf-8')));
    expect(computed).toEqual([]);
  });

  it('leaves the level to the band', () => {
    const allCss: string = readdirSync(resolve(here, '../../../styles'), {
      recursive: true, encoding: 'utf-8',
    })
      .filter((f: string) => f.endsWith('.css'))
      .map((f: string) => readFileSync(resolve(here, '../../../styles', f), 'utf-8'))
      .join('\n');
    const offenders: string[] = [];
    for (const cls of overlayClasses()) {
      const rule = new RegExp(
        `\\.${cls}(?![\\w-])[^\\s{,>+~]*(?:\\s*,[^{]*)?\\s*\\{([^}]*)\\}`, 'g',
      );
      for (const m of allCss.matchAll(rule)) {
        if (m[0].includes(':not(.modal-overlay)')) continue;
        if (/z-index:/.test(m[1])) offenders.push(cls);
      }
    }
    expect(offenders).toEqual([]);
  });
});
