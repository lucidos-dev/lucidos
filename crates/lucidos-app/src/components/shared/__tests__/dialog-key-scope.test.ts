import { describe, it, expect, beforeEach } from 'vitest';
import { dialogOwnsKey } from '../dialogKeyScope';
import { pushOverlay, _resetOverlayStackForTesting } from '../../../store/overlayStack';

// `ConfirmDialog` and `PromptDialog` render from independent signals, so both
// can be on screen at once, and each installs its own bubble-phase `document`
// keydown listener. `preventDefault()` does not stop a sibling listener on the
// same node, so one Enter typed into the prompt's input used to submit the
// prompt AND answer the confirm behind it with "yes" (a destructive action the
// reader never confirmed). `dialogOwnsKey` is the split.

/** A stand-in for a keydown target: `closest` walks a fixed ancestor chain.
 *  `dialogOwnsKey` duck-types on `closest`, so the fake needs nothing else. */
function targetInside(panel: unknown | null): EventTarget {
  return {
    closest: (selector: string) =>
      selector === '[data-overlay-panel]' ? (panel as Element | null) : null,
  } as unknown as EventTarget;
}

/** A stand-in for a real `<Overlay>` panel: reports its stack id the way the
 *  rendered node does, via `data-overlay-panel`. */
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

  // The bug: Enter in the prompt's input must not answer the confirm behind it.
  it('disowns a keystroke from another overlay panel', () => {
    expect(dialogOwnsKey(targetInside(other), own)).toBe(false);
  });

  // Clicking the dialog's own message text leaves focus on <body>, and a bare
  // Enter there must still answer the open dialog. A panel that cannot report a
  // stack id (this fake) falls back to answering.
  it('owns a keystroke that originated outside every overlay panel', () => {
    expect(dialogOwnsKey(targetInside(null), own)).toBe(true);
  });

  it('owns a keystroke with no target at all', () => {
    expect(dialogOwnsKey(null, own)).toBe(true);
  });

  // A target without `closest` (a non-Element event target, or a bare test
  // fake) reads as "outside every panel" rather than throwing.
  it('owns a keystroke whose target cannot be walked', () => {
    expect(dialogOwnsKey({} as EventTarget, own)).toBe(true);
  });

  // Before the panel ref resolves there is nothing to compare against; a
  // keystroke from another panel is still not ours.
  it('disowns another panel even before its own ref has resolved', () => {
    expect(dialogOwnsKey(targetInside(other), null)).toBe(false);
    expect(dialogOwnsKey(targetInside(null), null)).toBe(true);
  });
});

// The residual half of the same bug. Scoping by PANEL alone still double-fires
// when the keystroke belongs to no panel: click the confirm's own message text
// (focus lands on <body>), press Enter, and both stacked dialogs answered it.
// The reader saw the prompt close with their text AND the confirm behind it
// resolve `true`, committing the destructive action. Ownership of an
// outside-every-panel keystroke goes to the TOP overlay and to nobody else.
describe('dialogOwnsKey with a body-focused keystroke and a stacked overlay', () => {
  const confirmPanel = panelWithId('overlay-1');
  const promptPanel = panelWithId('overlay-2');

  function openConfirmThenPrompt() {
    pushOverlay({ id: 'overlay-1', dismiss: () => {} });
    pushOverlay({ id: 'overlay-2', dismiss: () => {} });
  }

  it('answers when it is the only overlay open', () => {
    pushOverlay({ id: 'overlay-1', dismiss: () => {} });
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
    pushOverlay({ id: 'overlay-1', dismiss: () => {} });
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(true);
  });

  // A keystroke from inside a panel is still resolved by panel identity; being
  // the top overlay does not let a dialog claim another panel's Enter.
  it('does not let the top overlay claim a keystroke from inside another panel', () => {
    openConfirmThenPrompt();
    expect(dialogOwnsKey(targetInside(confirmPanel), promptPanel)).toBe(false);
    expect(dialogOwnsKey(targetInside(promptPanel), promptPanel)).toBe(true);
  });

  // An empty stack should never cost the reader a working Enter key.
  it('answers when the stack is empty', () => {
    expect(dialogOwnsKey(targetInside(null), confirmPanel)).toBe(true);
  });
});
