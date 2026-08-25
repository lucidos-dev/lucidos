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
const gate: string = readFileSync(
  resolve(here, '../components/picker/PairingGate.tsx'),
  'utf8',
);

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
    // A handover statement at column 0 is the regression: it disarms the
    // watchdog before the picker chunk has landed. The `if (!IS_PICKER)` form
    // below is the one sanctioned unindented call, and the next test pins it.
    //
    // Both spellings are scanned. `handOverBootOwnership` is the one main.tsx
    // uses today, and the raw `__lucidosBootLoaded?.()` is what re-inlining the
    // helper here would bring back. Matching only the raw hook is how this guard
    // went vacuous the moment the helper moved to `utils/bootSplash.ts`.
    const unconditional = codeLines.filter(
      (l: string) =>
        /^\S/.test(l) &&
        /(handOverBootOwnership|__lucidosBootLoaded\?\.)\(\)/.test(l) &&
        !l.startsWith('if ('),
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

/**
 * The pairing screen is the third boot path, and it reaches neither of the two
 * above. `PairingGate` renders it INSTEAD of `WorkspacePicker`, so the lazy
 * loader that hands over never runs and the picker's `dismissBootSplash` never
 * fires. Both were missing when pairing shipped: the screen reloaded itself at
 * 15s, then sat under a full-viewport tap-to-retry splash that reloaded on a
 * click anywhere.
 */
describe('the pairing screen owns its own boot', () => {
  it('hands over once, and only for the unpaired state', () => {
    const calls = gate.match(/handOverBootOwnership\(\)/g) ?? [];
    expect(calls, 'exactly one handover site in the gate').toHaveLength(1);
    expect(
      /if \(state === 'unpaired'\) handOverBootOwnership\(\);/.test(gate),
      "gate the handover on 'unpaired': the other states still render the lazy picker",
    ).toBe(true);
  });

  it('dismisses the boot splash, which otherwise covers the form', () => {
    expect(
      /dismissBootSplash\(\)/.test(gate),
      'nothing else dismisses the splash on this path',
    ).toBe(true);
  });

  it('leaves no screen it can paint under the cover', () => {
    // One call per screen: the self-pair card, the install recipe, the form.
    // Each uncovers for itself, because a parent cannot see a child decide.
    const uses = gate.match(/useUncoverOnPaint\(/g) ?? [];
    expect(uses, 'one per screen the gate can paint, plus the definition').toHaveLength(4);
    expect(
      /dismissBootSplash\(\)/.test(gate.slice(gate.indexOf('function useUncoverOnPaint'))),
      'the hook is the only thing that lifts the cover',
    ).toBe(true);
    const raw = gate.match(/dismissBootSplash\(\)/g) ?? [];
    expect(raw, 'no screen may call it around the hook').toHaveLength(1);
  });

  it('uncovers on paint, not on mount, wherever a branch can draw nothing', () => {
    // Two screens hold the cover past a decision: the Tauri self-pair, and a
    // launch code redeeming on sight. Uncovering there would swap a covered
    // screen for a blank one. On the launch-code path it is worse than blank:
    // the form asks the user to pair a device already pairing.
    expect(gate).toMatch(/useUncoverOnPaint\(auto === 'running' && showBusy\);/);
    expect(gate).toMatch(/useUncoverOnPaint\(autoPair === 'done' \|\| showAutoPair\);/);
  });
});
