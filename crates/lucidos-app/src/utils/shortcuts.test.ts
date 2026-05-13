// Cmd+Shift+O is intercepted system-side on Mac (only Ctrl+Shift+O fires), and
// the historical `or C` suffix read as `or Cmd+Shift+C`. Mac tooltip must show
// `⌃⇧O` to match what actually works.

import { describe, it, expect, vi, beforeEach } from 'vitest';

const platformMocks = vi.hoisted(() => ({ isMac: false }));
vi.mock('./platform', () => ({
  get isMac() { return platformMocks.isMac; },
}));

beforeEach(() => {
  vi.resetModules();
});

describe('newThread shortcut tooltip', () => {
  it('Mac shows Ctrl+Shift+O (⌃⇧O), not Cmd+Shift+O (⌘⇧O), and drops the ambiguous "or C"', async () => {
    platformMocks.isMac = true;
    const { tooltipWithShortcut } = await import('./shortcuts');
    expect(tooltipWithShortcut('New thread', 'newThread')).toBe('New thread · ⌃⇧O');
  });

  it('non-Mac shows Ctrl+Shift+O', async () => {
    platformMocks.isMac = false;
    const { tooltipWithShortcut } = await import('./shortcuts');
    expect(tooltipWithShortcut('New thread', 'newThread')).toBe('New thread · Ctrl+Shift+O');
  });
});
