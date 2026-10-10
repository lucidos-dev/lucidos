// @vitest-environment jsdom
/**
 * The desktop title row follows the thread's title.
 *
 * A generated title and a rename both write `meta.title` on the thread object
 * in place. A signal-reading component skips a parent render whose props are
 * unchanged, so a row handed that same object kept its first title forever.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { DesktopThreadTitleBar } from '../ThreadView';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

const THREAD = 'thread-1';
let host: HTMLDivElement;

function shownTitle(): string {
  return host.querySelector('.thread-view-header .thread-title')?.textContent?.trim() ?? '';
}

beforeEach(() => {
  threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { meta: { title: 'Untitled Thread' } })]]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('DesktopThreadTitleBar', () => {
  it('shows a title written in place on the same thread object', async () => {
    await act(() => {
      render(<DesktopThreadTitleBar threadId={THREAD} />, host);
    });
    expect(shownTitle()).toBe('Untitled Thread');

    await act(() => {
      const thread = threadMap.value.get(THREAD)!;
      thread.meta.title = 'Renamed thread';
      threadMap.value = new Map(threadMap.value);
    });
    expect(shownTitle()).toBe('Renamed thread');
  });
});
