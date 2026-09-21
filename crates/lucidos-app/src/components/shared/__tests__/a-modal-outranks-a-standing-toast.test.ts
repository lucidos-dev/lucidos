import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { toastStackUrgency } from '../toastUrgency';
import { MAX_MODAL_STACK_DEPTH } from '../../../store/overlayStack';
import type { ToastItem, ToastType } from '../../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss: string = readFileSync(
  resolve(here, '../../../styles/components.css'), 'utf-8',
);
const baseCss: string = readFileSync(
  resolve(here, '../../../styles/global/base.css'), 'utf-8',
);
const toastSource: string = readFileSync(resolve(here, '../Toast.tsx'), 'utf-8');

/** A toast that waits to be answered, which is what `showToast` records. */
function standing(type: ToastType): ToastItem {
  return { id: 1, message: 'x', type, persistent: true, pane: 'thread' };
}

/** One on a timer: it is removed on its own schedule, seen or not. */
function timed(type: ToastType): ToastItem {
  return { id: 2, message: 'x', type, persistent: false, pane: 'thread' };
}

/**
 * The toast layer sat above every modal, which is right for a failure and
 * wrong for a standing offer.
 *
 * The report, the split between the two, and why lowering the whole layer is
 * the one thing that must not happen:
 * `docs/adr/0237-overlay-and-toast-paint-order.md`.
 */
describe('toastStackUrgency', () => {
  it('calls a stack of waiting offers standing', () => {
    expect(toastStackUrgency([standing('info'), standing('success')])).toBe('standing');
  });

  it('calls an empty stack standing', () => {
    expect(toastStackUrgency([])).toBe('standing');
  });

  it('turns urgent for an error, whatever else is stacked with it', () => {
    expect(toastStackUrgency([standing('info'), standing('error')])).toBe('urgent');
  });

  it('turns urgent for a warning too', () => {
    expect(toastStackUrgency([standing('warning')])).toBe('urgent');
  });

  /** The trap the type alone walks into. A plain `success` auto-dismisses after
   *  five seconds, so lowering it spends that timer under the scrim and the
   *  reader never learns the backup finished. */
  it('turns urgent for a TIMED toast, whatever its type says', () => {
    expect(toastStackUrgency([timed('success')])).toBe('urgent');
    expect(toastStackUrgency([timed('info')])).toBe('urgent');
  });

  /** The container is one stacking context, so the answer cannot be split. One
   *  toast that must be seen keeps the whole stack up. */
  it('keeps the whole stack up for one toast that cannot wait', () => {
    expect(toastStackUrgency([standing('info'), timed('success')])).toBe('urgent');
  });

  /** A toast stored before `persistent` existed carries none. Treating that as
   *  "may be lowered" would hide it, so the absent case reads as urgent. */
  it('treats a toast with no recorded persistence as urgent', () => {
    expect(toastStackUrgency([{ id: 3, message: 'x', type: 'info' }])).toBe('urgent');
  });
});

describe('the CSS the urgency drives', () => {
  /** The container is one stacking context, so a per-toast level cannot escape
   *  it. The attribute has to be on the container, and the render has to set
   *  it, or the rule below matches nothing. */
  it('is wired: the container carries the urgency the rule keys on', () => {
    expect(toastSource).toMatch(/data-toast-urgency=\{toastStackUrgency\(items\)\}/);
  });

  function standingRuleZ(): string {
    const m = componentsCss.match(
      /:root\[data-overlay-open\][^{]*\.toast-container\[data-toast-urgency="standing"\]\s*\{([^}]*)\}/,
    );
    expect(m, 'no standing-toast z-index rule in components.css').not.toBeNull();
    const z = m![1].match(/z-index:\s*([^;]+);/);
    expect(z, 'the standing-toast rule sets no z-index').not.toBeNull();
    return z![1].trim();
  }

  /** Below the FLOOR of the band, so one value clears every stacked modal at
   *  once. A modal resolves to `--z-modal + depth` (overlayStack.ts), and the
   *  tallest it reaches is the floor plus `MAX_MODAL_STACK_DEPTH`. */
  it('drops a standing stack below every modal the band can hold', () => {
    const floor = parseInt(baseCss.match(/--z-modal:\s*(\d+)\s*;/)![1], 10);
    const expr = standingRuleZ();
    const calc = expr.match(/^calc\(\s*var\(--z-modal\)\s*-\s*(\d+)\s*\)$/);
    expect(calc, `unexpected z-index form "${expr}"`).not.toBeNull();
    const lowered = floor - parseInt(calc![1], 10);
    expect(lowered).toBeLessThan(floor);
    expect(lowered).toBeLessThan(floor + MAX_MODAL_STACK_DEPTH);
    // Still above the header chrome, so it is not buried under the whole shell
    // when the overlay that lowered it draws no scrim.
    const controlPanel = parseInt(baseCss.match(/--z-control-panel:\s*(\d+)\s*;/)![1], 10);
    expect(lowered).toBeGreaterThan(controlPanel);
  });

  /** The blocker is not an `<Overlay>`, so it raises no `data-overlay-open` of
   *  its own. A modal left open when a client refresh starts would otherwise
   *  take the "Refreshing…" toast down with it. */
  it('stands down while the UI blocker is up, so the refresh toast survives', () => {
    expect(componentsCss).toMatch(
      /:root\[data-overlay-open\]:not\(\[data-ui-blocked\]\)\s*\.toast-container\[data-toast-urgency="standing"\]/,
    );
  });

  /** The whole point of the split. An urgent stack is never lowered, so no
   *  selector may match it. */
  it('never lowers an urgent stack', () => {
    expect(componentsCss).not.toMatch(/\.toast-container\[data-toast-urgency="urgent"\]/);
  });
});
