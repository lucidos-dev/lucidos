import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  _resetOverlayStackForTesting,
  MAX_MODAL_STACK_DEPTH,
  overlayStack,
  overlayStackDepth,
  pushOverlay,
  removeOverlay,
} from '../overlayStack';

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

  it('caps the depth so the band cannot climb past its ceiling', () => {
    for (let i = 0; i <= MAX_MODAL_STACK_DEPTH + 5; i++) pushOverlay(panel(`o${i}`));
    const top = `o${MAX_MODAL_STACK_DEPTH + 5}`;
    expect(overlayStackDepth(overlayStack.value, top)).toBe(MAX_MODAL_STACK_DEPTH);
  });
});
