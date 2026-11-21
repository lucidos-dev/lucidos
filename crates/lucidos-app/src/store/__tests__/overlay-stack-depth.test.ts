import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  _resetOverlayStackForTesting,
  MAX_MODAL_STACK_DEPTH,
  overlayStack,
  overlayStackDepth,
  pushOverlay,
  removeOverlay,
} from '../overlayStack';

/** **Paint order has to agree with the stack, or the contract lies.**
 *
 *  The stack already decides who answers Escape and who answers a pointer. It
 *  did not decide who is drawn on top, so an invisible modal could hold both.
 *  The report it shipped as, and the options weighed:
 *  `docs/adr/0237-overlay-and-toast-paint-order.md`. */
describe('overlayStackDepth', () => {
  beforeEach(_resetOverlayStackForTesting);

  const panel = (id: string) => ({ id, dismiss: vi.fn(), hasPanel: true });

  it('puts the first overlay on the floor of the band', () => {
    pushOverlay(panel('notice'));
    expect(overlayStackDepth(overlayStack.value, 'notice')).toBe(0);
  });

  it('paints a later-opened overlay above an earlier one', () => {
    pushOverlay(panel('notice'));
    pushOverlay(panel('confirm'));

    const notice = overlayStackDepth(overlayStack.value, 'notice');
    const confirm = overlayStackDepth(overlayStack.value, 'confirm');
    expect(confirm).toBeGreaterThan(notice);
  });

  /** `pushOverlay` runs in a layout effect, one render after the panel is
   *  first built. So an unknown id is an overlay about to be pushed on top.
   *  Answering the floor instead would paint its first frame under the modal
   *  it just opened over. */
  it('answers the top of the band for an id not on the stack yet', () => {
    pushOverlay(panel('notice'));
    expect(overlayStackDepth(overlayStack.value, 'confirm')).toBe(1);

    pushOverlay(panel('confirm'));
    expect(overlayStackDepth(overlayStack.value, 'confirm')).toBe(1);
  });

  it('closes the gap again when the overlay below is removed', () => {
    pushOverlay(panel('notice'));
    pushOverlay(panel('confirm'));
    removeOverlay('notice');
    expect(overlayStackDepth(overlayStack.value, 'confirm')).toBe(0);
  });

  /** The band has a ceiling, and `.ui-blocking-overlay` sits above it. Without
   *  the cap a deep stack climbs through the blocker that exists to cover it.
   *  `ui-blocking-overlay-z-index.test.ts` pins the CSS half. */
  it('caps the depth so the band cannot climb past its ceiling', () => {
    for (let i = 0; i <= MAX_MODAL_STACK_DEPTH + 5; i++) pushOverlay(panel(`o${i}`));
    const top = `o${MAX_MODAL_STACK_DEPTH + 5}`;
    expect(overlayStackDepth(overlayStack.value, top)).toBe(MAX_MODAL_STACK_DEPTH);
  });
});
