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
import { threadWidgets } from '../../../store/widgets';

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

  /** Every thread but Home offers Pin to Home beside Pin to shelf (ADR 0432). */
  it('offers Pin to Home in another thread, until the instance is on the board', () => {
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true } })]]);
    const pinnedTo = (apps: string[]) => {
      threadWidgets.value = new Map([['home', { status: 'loaded', data: apps.map((app_id) => (
        { app_id, name: app_id, reusable: true, reveal: 'on-load' as const, sizes: ['medium' as const], pinned: true }
      )) }]]);
    };
    pinnedTo([]);
    expect(labels({})).toEqual(expect.arrayContaining(['Pin to shelf', 'Pin to Home']));
    pinnedTo(['w']);
    expect(labels({})).not.toContain('Pin to Home');
    expect(labels({ threadId: 'home' })).not.toContain('Pin to shelf');
    threadMap.value = new Map();
    threadWidgets.value = new Map();
  });
});
