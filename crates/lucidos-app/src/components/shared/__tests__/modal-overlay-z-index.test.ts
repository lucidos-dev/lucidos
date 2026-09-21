import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
// global.css is an @import barrel; the :root design tokens live in the base partial.
const css = readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8');

/**
 * Regression test: a modal overlay (StepDetailModal, ScaleModal, …) MUST sit
 * above the floating header chrome so it actually
 * blocks the rest of the UI. Previous bug: --z-modal was 2000 while
 * --z-control-panel was 2200, so header buttons (compose, search, menu,
 * thread nav, title) punched through the dim backdrop. Hovering them
 * showed tooltips, clicking them ran their action instead of closing the
 * modal, and the modal failed its core promise of "next click outside
 * closes it".
 */
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

/**
 * An `overlayClass` may not name a z-index.
 *
 * `<Overlay>` writes an INLINE one on the same `.modal-overlay` element, the
 * modal's place in the band (`store/overlayStack.ts`), and inline beats a class
 * rule. A level named in the class is therefore dead.
 *
 * It is dead silently, which is why this is a scan. `.image-popup` carried
 * `calc(var(--z-control-panel) + 100)`, which resolves to the band floor: it
 * looked like it was working, and would have gone on looking like it until one
 * of the two tokens moved.
 */
describe('no backdrop overlayClass declares its own z-index', () => {
  const SRC: string = resolve(here, '../../..');

  const tsxFiles: string[] = readdirSync(SRC, { recursive: true, encoding: 'utf-8' })
    .filter((f: string) => f.endsWith('.tsx'));

  /** Every class handed to `<Overlay overlayClass=…>`, read off the source so a
   *  new overlay joins this check without being listed anywhere. */
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

  /** The scan can only read a literal, so a computed one would pass by being
   *  invisible. Fail on it instead of scanning less than the codebase has. */
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
      // The class has to be the LAST compound in its selector, since that is
      // what targets the `.modal-overlay` element itself. `.cls.other {` counts
      // and so does `.cls, .a {`. A DESCENDANT (`.cls .inner {`) does not: that
      // styles something inside the overlay, which may level itself freely.
      // `(?![\w-])` keeps `.cls-inner` from matching as `.cls`.
      const rule = new RegExp(
        `\\.${cls}(?![\\w-])[^\\s{,>+~]*(?:\\s*,[^{]*)?\\s*\\{([^}]*)\\}`, 'g',
      );
      for (const m of allCss.matchAll(rule)) {
        if (/z-index:/.test(m[1])) offenders.push(cls);
      }
    }
    expect(offenders).toEqual([]);
  });
});
