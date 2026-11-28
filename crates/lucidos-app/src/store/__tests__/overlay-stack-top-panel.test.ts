import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  _resetOverlayStackForTesting,
  dismissTopOverlay,
  pushOverlay,
  removeOverlay,
  topOverlay,
  topPanelOverlay,
} from '../overlayStack';

describe('topPanelOverlay', () => {
  beforeEach(_resetOverlayStackForTesting);

  const panel = (id: string) => ({ id, dismiss: vi.fn(), hasPanel: true });
  const escapeOnly = (id: string) => ({ id, dismiss: vi.fn(), hasPanel: false });

  it('is null while nothing is open', () => {
    expect(topPanelOverlay()).toBeNull();
  });

  it('sees through an Escape-only registrant to the panel below it', () => {
    pushOverlay(panel('menu'));
    pushOverlay(escapeOnly('model-picker-step'));

    expect(topOverlay()?.id).toBe('model-picker-step');
    expect(topPanelOverlay()?.id).toBe('menu');
  });

  it('returns null when only Escape-only registrants are on the stack', () => {
    pushOverlay(escapeOnly('pseudo-fullscreen'));
    pushOverlay(escapeOnly('thread-filter-panel'));
    expect(topOverlay()?.id).toBe('thread-filter-panel');
    expect(topPanelOverlay()).toBeNull();
  });

  it('hands the pointer back to the lower panel when the upper one closes', () => {
    pushOverlay(panel('modal'));
    pushOverlay(panel('confirm-dialog'));
    expect(topPanelOverlay()?.id).toBe('confirm-dialog');

    removeOverlay('confirm-dialog');
    expect(topPanelOverlay()?.id).toBe('modal');
  });

  it('agrees with topOverlay for a single panel', () => {
    pushOverlay(panel('only'));
    expect(topPanelOverlay()?.id).toBe(topOverlay()?.id);
  });

  it('leaves Escape dismissing the registrant above the panel', () => {
    const host = panel('menu');
    const step = escapeOnly('model-picker-step');
    pushOverlay(host);
    pushOverlay(step);

    expect(dismissTopOverlay()).toBe(true);
    expect(step.dismiss).toHaveBeenCalledTimes(1);
    expect(host.dismiss).not.toHaveBeenCalled();
  });
});
