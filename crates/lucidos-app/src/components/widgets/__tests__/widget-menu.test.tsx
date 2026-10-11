// @vitest-environment jsdom
/**
 * A built-in widget is read-only (ADR 0415), so its menu offers no reuse
 * change. Every other item stays.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { widgetMenuItems, type WidgetMenuTarget } from '../WidgetMenu';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

let host: HTMLElement | null = null;
function unmount(): void {
  if (host) render(null, host);
  host?.remove();
  host = null;
}
afterEach(unmount);

function labels(target: Partial<WidgetMenuTarget>): string[] {
  unmount();
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

  it('names Home, which draws no shelf, on a widget in Home', () => {
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true } })]]);
    expect(labels({ threadId: 'home' })).toContain('Pin to Home');
    expect(labels({ threadId: 'home', pinned: true })).toContain('Unpin from Home');
    threadMap.value = new Map();
  });
});
