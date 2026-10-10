// @vitest-environment jsdom
/**
 * A built-in widget is read-only (ADR 0415), so its menu offers no reuse
 * change. Every other item stays.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { widgetMenuItems, type WidgetMenuTarget } from '../WidgetMenu';

let host: HTMLElement | null = null;
afterEach(() => {
  if (host) render(null, host);
  host?.remove();
  host = null;
});

function labels(target: Partial<WidgetMenuTarget>): string[] {
  host = document.createElement('div');
  document.body.appendChild(host);
  const ctx = { run: (fn: () => void) => () => fn(), anchor: null };
  render(widgetMenuItems(ctx, {
    threadId: 't-1', appId: 'w', name: 'W', reusable: true, builtIn: false, pinned: false, ...target,
  }), host);
  return [...host.querySelectorAll('.widget-menu-label > span:first-child')].map((el) => el.textContent ?? '');
}

describe('widgetMenuItems', () => {
  it('offers Stop reusing on a reusable workspace widget', () => {
    expect(labels({})).toContain('Stop reusing');
  });

  it('offers no reuse change on a built-in widget', () => {
    const items = labels({ builtIn: true });
    expect(items).not.toContain('Stop reusing');
    expect(items).not.toContain('Make reusable');
    expect(items).toEqual(expect.arrayContaining(['Open in Canvas', 'Make app', 'Pin to shelf']));
  });
});
