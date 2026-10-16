// @vitest-environment jsdom
/**
 * A toast never moves keyboard focus when it appears.
 *
 * Most toasts arrive from a background poll or an SSE event, often while the
 * user is typing. Moving focus then would re-aim their next Enter at a button
 * they never looked at.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { ToastList, focusNewestToast } from '../Toast';
import { toasts, showToast } from '../../../store/store';

let host: HTMLDivElement;

beforeEach(() => {
  // Answer as a desktop with a real pointer and keyboard.
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
});

afterEach(() => {
  render(null, host);
  host.remove();
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

  it('skips a newer passive toast, which has nothing to act on', () => {
    showToast('Actionable.', 'warning', { action: { label: 'Retry', onClick: () => {} } });
    showToast('Saved.', 'success');
    render(<ToastList />, host);
    focusNewestToast();
    expect(document.activeElement?.textContent).toBe('Retry');
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
