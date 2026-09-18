// @vitest-environment jsdom
/**
 * A toast never takes focus off a field the user is typing in.
 *
 * An action toast focuses its default button as it appears, so Enter acts on it
 * without reaching for the mouse. The engine's "New version available" toast
 * pops out of a background poll, so it can land while the user is composing.
 * One that did turned the next Enter into "Switch to new version": the prompt
 * went unsent and the engine restarted.
 *
 * Rendered rather than poked through the pure slot picker. The steal is in the
 * ref callback that applies the slot, not in the choice of slot.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { ToastList } from '../Toast';
import { toasts, showToast } from '../../../store/store';

let host: HTMLDivElement;
let prompt: HTMLTextAreaElement;

beforeEach(() => {
  // The autofocus is gated on a hover-capable pointer, which jsdom denies by
  // default. Answer as a desktop so the behaviour under test can run at all.
  window.matchMedia = ((query: string) => ({
    matches: query.includes('hover: hover'),
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
  toasts.value = [];
  host = document.createElement('div');
  document.body.appendChild(host);
  prompt = document.createElement('textarea');
  document.body.appendChild(prompt);
});

afterEach(() => {
  render(null, host);
  host.remove();
  prompt.remove();
});

/** The reported toast: a neutral primary the autofocus would otherwise take. */
function showVersionToast(): void {
  showToast('New version available.', 'info', {
    secondaryAction: { label: 'Later', onClick: () => {} },
    action: { label: 'Switch to new version', onClick: () => {} },
  });
}

describe('toast autofocus', () => {
  it('leaves focus in the prompt when the toast lands mid-compose', () => {
    prompt.focus();
    showVersionToast();
    render(<ToastList />, host);
    expect(document.activeElement).toBe(prompt);
  });

  it('takes the default button when the user is not in a field', () => {
    showVersionToast();
    render(<ToastList />, host);
    expect(document.activeElement?.textContent).toBe('Switch to new version');
  });

  it('does not take it later, once it has stood down for a field', () => {
    // The decision is made once, as the toast mounts. A re-render churns the
    // ref, and a delayed steal would be as surprising as the original one.
    prompt.focus();
    showVersionToast();
    render(<ToastList />, host);
    prompt.blur();
    render(<ToastList />, host);
    expect(document.activeElement).toBe(document.body);
  });
});
