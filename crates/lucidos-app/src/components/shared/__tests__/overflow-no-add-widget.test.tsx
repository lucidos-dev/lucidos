// @vitest-environment jsdom
/**
 * I8 of the one-render plan: the thread ⋯ menu offers no Add widget. A
 * thread holds no pins, so a widget reaches it only through a reply's embed.
 */
import { beforeAll, describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadOverflowItemsChunk, ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

beforeAll(() => ThreadOverflowItemsChunk.preload());

let host: HTMLElement | null = null;

afterEach(() => {
  if (host) { render(null, host); host.remove(); host = null; }
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  threadMap.value = new Map();
});

describe('The thread overflow menu', () => {
  it('offers no Add widget on a sent thread', () => {
    threadMap.value = new Map([['t', makeThreadState('t')]]);
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId="t" title="T" />, host!); });
    act(() => { host!.querySelector<HTMLElement>('button[aria-haspopup="menu"]')?.click(); });
    const labels = [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')]
      .map((el) => el.textContent?.trim());
    expect(labels).toContain('Copy thread reference');
    expect(labels.some((l) => l?.startsWith('Add widget'))).toBe(false);
  });
});
