// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { render } from 'preact';
import { widgetMenuItems, type WidgetMenuTarget } from '../WidgetMenu';

function labels(target: WidgetMenuTarget): string[] {
  const host = document.createElement('div');
  const run = (fn: () => void) => () => fn();
  render(widgetMenuItems({ run } as unknown as Parameters<typeof widgetMenuItems>[0], target), host);
  return Array.from(host.querySelectorAll('.widget-menu-label > span:first-child')).map((el) => el.textContent ?? '');
}

const BASE: WidgetMenuTarget = { threadId: 't', appId: 'board', name: 'Board', reusable: true, builtIn: false, sizes: [] };

describe('the widget menu', () => {
  it('offers Stop reusing for a thread widget', () => {
    expect(labels(BASE)).toContain('Stop reusing');
  });

  it('offers no Stop reusing for a plugin widget', () => {
    expect(labels({ ...BASE, originPluginId: 'habit-tracker' })).not.toContain('Stop reusing');
  });
});
