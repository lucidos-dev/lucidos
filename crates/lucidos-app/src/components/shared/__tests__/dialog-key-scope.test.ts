import { describe, it, expect, beforeEach } from 'vitest';
import { dialogOwnsKey } from '../dialogKeyScope';
import { pushOverlay, _resetOverlayStackForTesting } from '../../../store/overlayStack';


function targetInside(panel: unknown | null): EventTarget {
  return {
    closest: (selector: string) =>
      selector === '[data-overlay-panel]' ? (panel as Element | null) : null,
  } as unknown as EventTarget;
}

function panelWithId(id: string): HTMLElement {
  return {
    getAttribute: (name: string) => (name === 'data-overlay-panel' ? id : null),
  } as unknown as HTMLElement;
}

beforeEach(() => {
  _resetOverlayStackForTesting();
});

describe('dialogOwnsKey', () => {
  const own = { id: 'own-panel' } as unknown as HTMLElement;
  const other = { id: 'other-panel' } as unknown as HTMLElement;

  it('owns a keystroke from inside its own panel', () => {
    expect(dialogOwnsKey(targetInside(own), own)).toBe(true);
  });

  it('disowns a keystroke from another overlay panel', () => {
    expect(dialogOwnsKey(targetInside(other), own)).toBe(false);
  });

  it('owns a keystroke that originated outside every overlay panel', () => {
    expect(dialogOwnsKey(targetInside(null), own)).toBe(true);
  });

  it('owns a keystroke with no target at all', () => {
    expect(dialogOwnsKey(null, own)).toBe(true);
  });

  it('owns a keystroke whose target cannot be walked', () => {
    expect(dialogOwnsKey({} as EventTarget, own)).toBe(true);
  });

  it('disowns another panel even before its own ref has resolved', () => {
    expect(dialogOwnsKey(targetInside(other), null)).toBe(false);
    expect(dialogOwnsKey(targetInside(null), null)).toBe(true);
  });
});

describe('dialogOwnsKey with a body-focused keystroke and a stacked overlay', () => {
  const confirmPanel = panelWithId('overlay-1');
  const promptPanel = panelWithId('overlay-2');

  function openConfirmThenPrompt() {
    pushOverlay({ id: 'overlay-1', dismiss: () => {}, hasPanel: true });
    pushOverlay({ id: 'overlay-2', dismiss: () => {}, hasPanel: true });
  }

  it('answers when it is the only overlay open', () => {
    pushOverlay({ id: 'overlay-1', dismiss: () => {}, hasPanel: true });
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(true);
  });

  it('gives the keystroke to the top overlay and to nobody else', () => {
    openConfirmThenPrompt();
    expect(dialogOwnsKey(targetInside(null), promptPanel)).toBe(true);
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(false);
  });

  it('follows the stack when the top overlay closes', () => {
    openConfirmThenPrompt();
    _resetOverlayStackForTesting();
    pushOverlay({ id: 'overlay-1', dismiss: () => {}, hasPanel: true });
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(true);
  });

  it('does not let the top overlay claim a keystroke from inside another panel', () => {
    openConfirmThenPrompt();
    expect(dialogOwnsKey(targetInside(confirmPanel), promptPanel)).toBe(false);
    expect(dialogOwnsKey(targetInside(promptPanel), promptPanel)).toBe(true);
  });

  it('answers when the stack is empty', () => {
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(true);
  });
});
