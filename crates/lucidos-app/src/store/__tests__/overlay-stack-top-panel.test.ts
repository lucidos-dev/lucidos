import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  _resetOverlayStackForTesting,
  dismissTopOverlay,
  pushOverlay,
  removeOverlay,
  topOverlay,
  topPanelOverlay,
} from '../overlayStack';

/** **Two questions, two answers, and the stack must keep them apart.**
 *
 *  Escape asks for the top entry. An Escape-only registrant is there to answer
 *  the key, and sitting above its host panel is what makes a step answer before
 *  the panel closes.
 *
 *  A POINTER asks for the top panel. A registrant draws nothing, so no click can
 *  be meant for it. Answering the raw top there switched a panel's outside-click
 *  dismiss off whenever a step was open inside it. That is every model menu in
 *  the app, since `ModelSelectionPicker` pushes its step above its host.
 *
 *  Driven against the real stack rather than a hand-written predicate, which is
 *  what the `makeDismissHandlers` unit tests use and why they could not see it. */
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

    // Escape still lands on the step, which is the whole point of pushing it.
    expect(topOverlay()?.id).toBe('model-picker-step');
    // The pointer still belongs to the menu, the only thing actually drawn.
    expect(topPanelOverlay()?.id).toBe('menu');
  });

  it('returns null when only Escape-only registrants are on the stack', () => {
    pushOverlay(escapeOnly('pseudo-fullscreen'));
    pushOverlay(escapeOnly('thread-filter-panel'));
    expect(topOverlay()?.id).toBe('thread-filter-panel');
    expect(topPanelOverlay()).toBeNull();
  });

  /** The case the gate exists for: a modal opened from a popover. Only the
   *  modal may answer a click, and closing it hands the pointer back. */
  it('hands the pointer back to the lower panel when the upper one closes', () => {
    pushOverlay(panel('waiting-panel'));
    pushOverlay(panel('condition-modal'));
    expect(topPanelOverlay()?.id).toBe('condition-modal');

    removeOverlay('condition-modal');
    expect(topPanelOverlay()?.id).toBe('waiting-panel');
  });

  /** A lone overlay is always both, which is what keeps every existing caller
   *  behaving exactly as it did. */
  it('agrees with topOverlay for a single panel', () => {
    pushOverlay(panel('only'));
    expect(topPanelOverlay()?.id).toBe(topOverlay()?.id);
  });

  /** `dismissTopOverlay` is Escape's path and stays on the raw top. */
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
