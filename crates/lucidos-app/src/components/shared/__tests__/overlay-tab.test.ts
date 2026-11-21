// @vitest-environment jsdom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { overlayStack, pushOverlay, _resetOverlayStackForTesting } from '../../../store/overlayStack';
import { handleOverlayTab, overlayTabAction, shouldRestoreFocus } from '../overlayFocus';

// The shell behind an overlay is inert only to the pointer, so Tab walked
// straight into it: open a step's detail modal from the keyboard and Tab went on
// through the transcript underneath.
describe('overlayTabAction', () => {
  it('a popover closes when Tab starts outside it, such as on its combobox input', () => {
    // A free-text dropdown opens on focus. Tabbing past it left the menu open,
    // and a Settings page collected a stack of them.
    expect(overlayTabAction({ modal: false, activeInPanel: false, count: 3, target: 0 })).toEqual({ kind: 'dismiss' });
  });

  it('a dialog takes the step target, from outside or inside', () => {
    expect(overlayTabAction({ modal: true, activeInPanel: false, count: 3, target: 0 })).toEqual({ kind: 'focus', index: 0 });
    expect(overlayTabAction({ modal: true, activeInPanel: true, count: 3, target: 2 })).toEqual({ kind: 'focus', index: 2 });
  });

  it('a dialog with nothing to tab to keeps the key', () => {
    expect(overlayTabAction({ modal: true, activeInPanel: false, count: 0, target: null })).toEqual({ kind: 'stay' });
  });

  it('an in-between step inside the panel stays native', () => {
    expect(overlayTabAction({ modal: false, activeInPanel: true, count: 3, target: null })).toEqual({ kind: 'native' });
  });
});

describe('handleOverlayTab', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    _resetOverlayStackForTesting();
  });

  const tab = (shiftKey = false) => new KeyboardEvent('keydown', { key: 'Tab', shiftKey, cancelable: true });

  function mount(html: string): void {
    document.body.innerHTML = html;
    // jsdom lays nothing out; give every control a box.
    for (const el of document.querySelectorAll<HTMLElement>('*')) {
      el.getClientRects = () => [{}] as unknown as DOMRectList;
    }
  }

  it('pulls focus from the shell into the top dialog', () => {
    mount(`
      <div class="pane-thread"><button data-id="row">Step</button></div>
      <div class="modal-overlay"><div data-overlay-panel="d" data-overlay-modal>
        <button data-id="close">Close</button><button>Copy</button>
      </div></div>`);
    pushOverlay({ id: 'd', dismiss: () => {}, hasPanel: true });
    document.querySelector<HTMLElement>('[data-id="row"]')!.focus();

    const e = tab();
    expect(handleOverlayTab(e)).toBe(true);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(document.querySelector('[data-id="close"]'));
  });

  it('closes a popover the focus is not in, then lets the pane trap run', () => {
    mount(`
      <div class="pane-content"><input data-id="combo"></div>
      <div data-overlay-panel="menu"><button>One</button></div>`);
    const dismiss = vi.fn();
    pushOverlay({ id: 'menu', dismiss, hasPanel: true });
    document.querySelector<HTMLElement>('[data-id="combo"]')!.focus();

    const e = tab();
    expect(handleOverlayTab(e)).toBe(false);
    expect(dismiss).toHaveBeenCalledOnce();
    expect(e.defaultPrevented).toBe(false);
  });

  it('after closing a popover, the dialog under it still contains the Tab', () => {
    // A dropdown inside a dialog: its input sits in the dialog, outside the menu.
    mount(`
      <div class="modal-overlay"><div data-overlay-panel="d" data-overlay-modal>
        <input data-id="combo"><button data-id="ok">OK</button>
      </div></div>
      <div data-overlay-panel="menu"><button>One</button></div>`);
    pushOverlay({ id: 'd', dismiss: () => {}, hasPanel: true });
    const dismiss = vi.fn();
    pushOverlay({ id: 'menu', dismiss, hasPanel: true });
    document.querySelector<HTMLElement>('[data-id="ok"]')!.focus();

    const e = tab();
    expect(handleOverlayTab(e)).toBe(true);
    expect(dismiss).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(document.querySelector('[data-id="combo"]'));
  });

  it('keeps Tab inside a dialog whose body the reader clicked', () => {
    // A click on dialog prose focuses its tabindex=-1 body. Native Tab from
    // there walked out into the shell behind.
    mount(`
      <div class="modal-overlay"><div data-overlay-panel="d" data-overlay-modal>
        <button data-id="close">Close</button><div tabindex="-1" data-id="body">Text</div>
      </div></div>
      <div class="pane-thread"><button>Behind</button></div>`);
    pushOverlay({ id: 'd', dismiss: () => {}, hasPanel: true });
    document.querySelector<HTMLElement>('[data-id="body"]')!.focus();

    const e = tab();
    expect(handleOverlayTab(e)).toBe(true);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(document.querySelector('[data-id="close"]'));
  });

  it('ignores an Escape-only registrant, which draws nothing', () => {
    mount('<div class="pane-thread"><button>Row</button></div>');
    pushOverlay({ id: 'step', dismiss: () => {}, hasPanel: false });
    expect(handleOverlayTab(tab())).toBe(false);
    expect(overlayStack.value).toHaveLength(1);
  });
});

describe('shouldRestoreFocus', () => {
  const base = { focusLost: true, openerUsable: true, openerPane: 'thread', markerPane: 'thread' };

  it('returns focus to the opener when it went down with the panel', () => {
    expect(shouldRestoreFocus(base)).toBe(true);
  });

  it('leaves focus that the overlay action put somewhere on purpose', () => {
    expect(shouldRestoreFocus({ ...base, focusLost: false })).toBe(false);
  });

  it('respects an action that sent the reader to another pane', () => {
    // A menu item that opened a file moved the marker to the content pane.
    expect(shouldRestoreFocus({ ...base, markerPane: 'content' })).toBe(false);
  });

  it('restores an opener outside every pane, such as a header button', () => {
    expect(shouldRestoreFocus({ ...base, openerPane: null, markerPane: 'content' })).toBe(true);
  });

  it('never targets an opener that is gone', () => {
    expect(shouldRestoreFocus({ ...base, openerUsable: false })).toBe(false);
  });
});
