import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../Overlay.tsx'), 'utf-8');
const SRC_ROOT: string = resolve(here, '../../..');

// The whole point of <Overlay> is that the click-outside-dismiss contract is
// centralized so no individual overlay can drop or mis-wire it (the
// SearchEverywhere `anchor=null` / touch-reopen bug). These are tripwires: if a
// future edit guts the contract, one of these fails loudly. The behavior itself
// is exercised end-to-end by `e2e/search-everywhere-close-mobile.spec.ts` (the
// migrated reference overlay).
describe('Overlay — centralizes the dismiss contract', () => {
  it('delegates outside-click dismiss + swallow to useDismissOnOutside, forwarding the anchor', () => {
    // Must call the canonical hook (not a hand-rolled listener) and pass the
    // anchor through — never a hardcoded null, which would treat a toggle button
    // as "outside" and reopen it on touch.
    expect(source).toMatch(/useDismissOnOutside\(\s*open\s*,\s*panelRef\s*,\s*anchor\s*,\s*onClose\s*[,)]/);
  });

  /** **Only the topmost PANEL answers an outside click.** Every open overlay
   *  installs its own document listener, and each asks only whether the target
   *  is outside ITS panel. So a SIBLING overlay's panel reads as outside, and
   *  the lower one dismissed itself behind the upper one. Escape already went
   *  LIFO through the stack, and the pointer has to agree. */
  it('gates the outside-click dismiss on being the top panel of the overlay stack', () => {
    // Bounded `[\s\S]` rather than `[^)]`, which cannot cross the `()` in the
    // predicate's own arrow.
    expect(source).toMatch(/useDismissOnOutside\([\s\S]{0,160}?topPanelOverlay\(\)\?\.id === idRef\.current/);
    // The topmost PANEL, never the raw stack top: an Escape-only registrant
    // draws nothing, and one sits above its own host panel by design.
    expect(source).not.toMatch(/\btopOverlay\(\)/);
  });

  /** The registration the gate reads must land in the SAME commit as the
   *  listeners, which install pre-paint. A plain `useEffect` here left the
   *  overlay's first frame with live listeners that answered nothing. */
  it('registers into the stack pre-paint, alongside the dismiss listeners', () => {
    expect(source).toMatch(/useLayoutEffect\(\(\) => \{[\s\S]{0,200}?pushOverlay\(/);
  });

  /** **`hasPanel` is what `topPanelOverlay` counts.** Its PRESENCE is a compile
   *  error to omit, since `OverlayEntry` requires it. Its VALUE is not, so that
   *  is what this pins. `false` here would answer null for every stack state.
   *  The dismiss, the swallow, the click fallback and the fallback Escape would
   *  switch off at once, on every overlay in the app. */
  it('marks its stack entry as owning a panel', () => {
    expect(source).toMatch(/pushOverlay\([\s\S]{0,120}?hasPanel: true/);
  });

  it('routes Escape through the central overlay stack, not a per-instance keydown listener', () => {
    expect(source).toMatch(/pushOverlay\(/);
    expect(source).toMatch(/removeOverlay\(/);
  });

  it('never hand-rolls its own document dismiss listener', () => {
    // A bare addEventListener('pointerdown'|'mousedown'|'click', …) is exactly
    // the anti-pattern the central component exists to eliminate.
    expect(source).not.toMatch(/addEventListener\(\s*['"](?:pointerdown|mousedown|click)['"]/);
  });
});

/** **An `onClose` must not RETURN the assignment that closes the overlay.**
 *
 *  `makeDismissHandlers` reads a `false` return as "that call was a no-op". It
 *  then leaves the paired-click suppressor disarmed on purpose, so the user's
 *  tap still reaches its target. An arrow with an expression body returns its
 *  expression, so `onClose={() => (open.value = false)}` hands back exactly
 *  that signal. The overlay closes and the control underneath the dismissing
 *  click fires with it, which is the one thing the whole contract exists to
 *  prevent. Four overlays shipped that way, because `useDismissOnOutside`'s own
 *  doc comment recommended the form.
 *
 *  A source scan, because it is a per-callsite shape no type can catch: the
 *  prop is typed `() => void | boolean` and `false` is a legal, meaningful
 *  value. Braces are the fix, `() => { open.value = false; }`. */
describe('every onClose keeps its no-op signal to itself', () => {
  const overlayUsers: string[] = readdirSync(SRC_ROOT, { recursive: true, encoding: 'utf-8' })
    .filter((f: string) => f.endsWith('.tsx') || f.endsWith('.ts'))
    .map((f: string) => resolve(SRC_ROOT, f));

  // An arrow whose body is an assignment, in the two spellings that compile:
  // `() => (x.y = false)` and the bare `() => x.y = false`. Both evaluate to
  // the assigned value. It anchors on `=>` rather than on `onClose=`, so it
  // also catches a handler hoisted into a named const and passed as
  // `onClose={close}`. The first version of this scan missed both shapes.
  const ASSIGNMENT_ARROW = /\(\s*\)\s*=>\s*\(?\s*[A-Za-z_$][\w$.]*\s*=\s*(?:false|null|undefined|0|''|"")\s*\)?/;

  it('never gives a dismiss callback an expression body that yields the assigned value', () => {
    const offenders: string[] = [];
    for (const file of overlayUsers) {
      const text: string = readFileSync(file, 'utf-8');
      for (const line of text.split('\n')) {
        // Skip comments, so this guard's own doc quoting the bad form, and any
        // other prose about it, is not read as a call site.
        const trimmed: string = line.trim();
        if (trimmed.startsWith('*') || trimmed.startsWith('//') || trimmed.startsWith('/*')) continue;
        // Only lines that are plausibly a dismiss callback or its definition.
        if (!/onClose|onDismiss|\bdismiss\b|close/i.test(line)) continue;
        if (ASSIGNMENT_ARROW.test(line)) {
          offenders.push(`${file.slice(SRC_ROOT.length + 1)}: ${line.trim()}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});
