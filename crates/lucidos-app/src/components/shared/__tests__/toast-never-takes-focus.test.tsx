// @vitest-environment jsdom
/**
 * A toast never moves keyboard focus when it appears.
 *
 * Most toasts arrive from a background poll or an SSE event, often while the
 * user is typing. Moving focus then would re-aim their next Enter at a button
 * they never looked at.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { Toast, ToastList, focusNewestToast } from '../Toast';
import { toasts, showToast } from '../../../store/store';
import { focusPaneMainControl } from '../../layout/paneFocus';
import { pushOverlay, _resetOverlayStackForTesting } from '../../../store/overlayStack';

vi.mock('../../layout/paneFocus', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../layout/paneFocus')>(),
  focusPaneMainControl: vi.fn(),
}));

let host: HTMLDivElement;
let hoverPointer: boolean;

beforeEach(() => {
  hoverPointer = true;
  // Answer as a desktop with a real pointer and keyboard.
  window.matchMedia = ((query: string) => ({
    matches: hoverPointer && query.includes('hover: hover'),
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
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('[data-overlay-panel]').forEach((panel) => panel.remove());
  document.documentElement.removeAttribute('data-overlay-open');
  _resetOverlayStackForTesting();
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe('focused toast expiry', () => {
  it.each([undefined, 'saved'])('moves to the remaining toast after expiry (key=%s)', (key) => {
    vi.useFakeTimers();
    showToast('Retry the operation.', 'warning', { action: { label: 'Retry', onClick: () => {} } });
    showToast('Saved.', 'success', { key, autoDismissMs: 1000 });
    act(() => render(<Toast />, host));
    focusNewestToast();
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Dismiss');
    act(() => { vi.advanceTimersByTime(1000); });
    expect(document.activeElement?.textContent).toBe('Retry');
  });

  it('returns to the pane when the last focused toast expires', () => {
    vi.useFakeTimers();
    showToast('Saved.', 'success', { autoDismissMs: 1000 });
    act(() => render(<Toast />, host));
    focusNewestToast();
    act(() => { vi.advanceTimersByTime(1000); });
    expect(focusPaneMainControl).toHaveBeenCalledOnce();
  });

  it.each([
    { hasControl: true, hover: true },
    { hasControl: false, hover: true },
    { hasControl: true, hover: false },
  ])('restores overlay focus only on keyboard devices ($hasControl, $hover)', ({ hasControl, hover }) => {
    hoverPointer = hover;
    vi.useFakeTimers();
    const panel = document.createElement('div');
    panel.dataset.overlayPanel = 'dialog';
    panel.tabIndex = -1;
    const button = document.createElement('button');
    button.textContent = 'Continue';
    button.getClientRects = () => [{ width: 100, height: 30 }] as unknown as DOMRectList;
    if (hasControl) panel.appendChild(button);
    document.body.appendChild(panel);
    pushOverlay({ id: 'dialog', hasPanel: true, dismiss: () => {} });
    // An Escape-only step must not shadow the panel that should regain focus.
    pushOverlay({ id: 'step', hasPanel: false, dismiss: () => {} });
    document.documentElement.setAttribute('data-overlay-open', '');
    showToast('Background status.', 'info', { key: 'background-status' });
    showToast('Failed.', 'error', { autoDismissMs: 1000 });
    act(() => render(<Toast />, host));
    focusNewestToast();
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Dismiss');
    act(() => { vi.advanceTimersByTime(1000); });
    expect(document.activeElement).toBe(hover ? (hasControl ? button : panel) : document.body);
    expect(focusPaneMainControl).not.toHaveBeenCalled();
  });

  it('leaves focus in a field when an unfocused toast expires', () => {
    vi.useFakeTimers();
    const field = document.createElement('textarea');
    document.body.appendChild(field);
    showToast('Saved.', 'success', { autoDismissMs: 1000 });
    act(() => render(<Toast />, host));
    field.focus();
    act(() => { vi.advanceTimersByTime(1000); });
    expect(document.activeElement).toBe(field);
    expect(focusPaneMainControl).not.toHaveBeenCalled();
    field.remove();
  });
});

describe('toast focus', () => {
  it('leaves focus where it was for a two-button toast', () => {
    showToast('New version available.', 'info', {
      secondaryAction: { label: 'Later', onClick: () => {} },
      action: { label: 'Switch', onClick: () => {} },
    });
    render(<ToastList />, host);
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus where it was for a card-tap toast', () => {
    showToast('Something happened.', 'info', { action: { label: 'Open', onClick: () => {} } });
    render(<ToastList />, host);
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus in a field the user is typing in', () => {
    const prompt = document.createElement('textarea');
    document.body.appendChild(prompt);
    prompt.focus();
    showToast('Delete it?', 'warning', {
      action: { label: 'Delete', onClick: () => {}, variant: 'danger' },
    });
    render(<ToastList />, host);
    expect(document.activeElement).toBe(prompt);
    prompt.remove();
  });
});

describe('focusNewestToast', () => {
  it('lands on the newest toast, which the keyboard reaches no other way', () => {
    showToast('Older.', 'warning', { action: { label: 'Retry', onClick: () => {} } });
    showToast('Newer.', 'warning', {
      secondaryAction: { label: 'Later', onClick: () => {} },
      action: { label: 'Switch', onClick: () => {} },
    });
    render(<ToastList />, host);
    focusNewestToast();
    expect(document.activeElement?.textContent).toBe('Later');
    expect(document.activeElement?.closest('.toast')?.textContent).toContain('Newer.');
  });

  it('reaches the close X on a newer timed toast', () => {
    showToast('Actionable.', 'warning', { action: { label: 'Retry', onClick: () => {} } });
    showToast('Saved.', 'success');
    render(<ToastList />, host);
    focusNewestToast();
    expect(document.activeElement?.getAttribute('aria-label')).toBe('Dismiss');
    expect(document.activeElement?.closest('.toast')?.textContent).toContain('Saved.');
  });

  it('stays out of a standing toast drawn under an open overlay', () => {
    showToast('Later.', 'info', { action: { label: 'Open', onClick: () => {} }, secondaryAction: { label: 'Later', onClick: () => {} } });
    render(<ToastList />, host);
    document.documentElement.setAttribute('data-overlay-open', '');
    try {
      focusNewestToast();
      expect(document.activeElement).toBe(document.body);
    } finally {
      document.documentElement.removeAttribute('data-overlay-open');
    }
  });

  it('still reaches an urgent toast, which paints over the overlay', () => {
    showToast('Failed.', 'error', { action: { label: 'Retry', onClick: () => {} } });
    render(<ToastList />, host);
    document.documentElement.setAttribute('data-overlay-open', '');
    try {
      focusNewestToast();
      expect(document.activeElement?.textContent).toBe('Retry');
    } finally {
      document.documentElement.removeAttribute('data-overlay-open');
    }
  });

  it('does nothing when no toast is on screen', () => {
    render(<ToastList />, host);
    focusNewestToast();
    expect(document.activeElement).toBe(document.body);
  });
});
