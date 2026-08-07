import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

/**
 * Pins the boot-ownership handover contract between `index.html`'s inline
 * watchdog and `main.tsx`.
 *
 * `__lucidosBootLoaded()` IS the handover: it clears the watchdog's 15s stall
 * timer and its entry-script error listener, so every boot failure after that
 * call is the application's to recover. The watchdog is the ONLY recovery a
 * PICKER document has, because `revealGatewayEscape` offers its link to
 * direct-port documents alone, leaving the picker with the inline retry-once
 * plus tap-to-retry (the only way back for an iOS PWA, which has no reload
 * button).
 *
 * So the moment the picker's root moved into its own lazy chunk, an
 * unconditional top-level handover became a real regression: it disarms the
 * watchdog while the thing it guards is still being fetched, and
 * `lazyComponent`'s own stale-chunk reload does not cover the gap (it stands
 * down when sessionStorage is unavailable or it already reloaded within 30s,
 * and its fallback toast cannot render on a document whose only component is
 * the chunk that failed).
 *
 * The test infra has no jsdom and `main.tsx` renders on import, so this is a
 * source-scan rather than a behavioural test, following the precedent in
 * `components/shared/__tests__/skeleton-guard.test.ts`.
 */

const here = dirname(fileURLToPath(import.meta.url));
const src: string = readFileSync(resolve(here, '../main.tsx'), 'utf8');

/** Source lines with `//` comments stripped, so the prose explaining a rule
 *  cannot be what satisfies the rule. */
const codeLines: string[] = src
  .split('\n')
  .map((l: string) => l.replace(/\/\/.*$/, ''))
  .filter((l: string) => l.trim() !== '');

/** The `lazyComponent(() => import('./components/picker/WorkspacePicker')…)`
 *  expression, from the loader's dynamic import to the end of that statement. */
function pickerLoaderBody(): string | null {
  const at = src.indexOf("import('./components/picker/WorkspacePicker')");
  if (at === -1) return null;
  const end = src.indexOf('\n);', at);
  return end === -1 ? src.slice(at) : src.slice(at, end);
}

describe('boot ownership handover', () => {
  it('keeps the picker code-split (the premise of the rest of this suite)', () => {
    expect(pickerLoaderBody(), 'the picker should load via lazyComponent(() => import(...))').not.toBeNull();
    expect(src).toMatch(/const WorkspacePicker = lazyComponent\(/);
  });

  it('never hands over unconditionally at the top level', () => {
    // A bare `__lucidosBootLoaded?.()` statement at column 0 is the regression:
    // it disarms the watchdog before the picker chunk has landed. Inside the
    // `handOverBootOwnership` body the call is indented, so only an unindented
    // one is a top-level statement.
    const unconditional = codeLines.filter(
      (l: string) => /__lucidosBootLoaded\?\.\(\)/.test(l) && /^\S/.test(l),
    );
    expect(
      unconditional,
      'gate the top-level handover: the picker path hands over from its lazy loader instead',
    ).toEqual([]);
  });

  it('hands over eagerly only on the non-picker path', () => {
    expect(
      codeLines.some((l: string) => /^if \(!IS_PICKER\) handOverBootOwnership\(\);$/.test(l.trim())),
      'main.tsx should call handOverBootOwnership() eagerly only when !IS_PICKER',
    ).toBe(true);
  });

  it('hands over from inside the picker lazy loader', () => {
    expect(
      /handOverBootOwnership\(\);/.test(pickerLoaderBody() ?? ''),
      'the picker path must hand over once its own chunk resolves',
    ).toBe(true);
  });
});
