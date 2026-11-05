// @vitest-environment jsdom
/**
 * Which glyph each surface wears. Current is the stacked layers, on its drawer
 * header and on both "Move to Current" actions. The inbox tray belongs to the
 * Ongoing grouping, which sits in the same header, so the two must never match.
 */
import { beforeAll, describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, type ComponentChild } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer, setSectionCollapsed } from '../ThreadDrawer';
import { ThreadGroupingButton } from '../../layout/ThreadsHeaderControls';
import { ThreadOverflowItemsChunk, ThreadOverflowMenu } from '../../shared/ThreadOverflowMenu';
import { getWaitingState } from '../../chat/WaitingBanner';
import { CurrentIcon, InboxIcon } from '../../shared/icons';
import { threadMap, threadsLoaded, focusedThreadId, setDrawerGrouping } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

// The menu's rows are code-split. Warm them, so a click opens it at once.
beforeAll(() => ThreadOverflowItemsChunk.preload());

vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

let host: HTMLDivElement;

function markup(node: ComponentChild): string {
  const box = document.createElement('div');
  render(node, box);
  return box.innerHTML;
}

function mount(node: ComponentChild) {
  act(() => { render(node, host); });
}

beforeEach(() => {
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  localStorage.clear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
  threadMap.value = new Map();
  focusedThreadId.value = null;
  vi.unstubAllGlobals();
});

describe('the Current icon', () => {
  it('is not the Ongoing inbox tray', () => {
    expect(markup(<CurrentIcon />)).not.toBe(markup(<InboxIcon />));
  });

  it('heads the Current drawer section', () => {
    setSectionCollapsed('current', false);
    threadMap.value = new Map([['a', makeThreadState('a', { meta: { title: 'Current one', section: 'inbox' } })]]);
    threadsLoaded.value = true;
    mount(<ThreadDrawer forceVisible />);
    const header = Array.from(host.querySelectorAll<HTMLElement>('.list-section-title-collapsible'))
      .find(h => h.querySelector('.section-label')?.textContent === 'Current')!;
    expect(header.querySelector('.section-icon')!.innerHTML).toBe(markup(<CurrentIcon size="0.875rem" />));
  });

  it('marks Move to Current in the thread overflow menu', () => {
    threadMap.value = new Map([['t', makeThreadState('t', { meta: { section: 'archived' } })]]);
    mount(<ThreadOverflowMenu threadId="t" title="T" />);
    act(() => { host.querySelector<HTMLElement>('button[aria-haspopup="menu"]')!.click(); });
    const item = Array.from(document.querySelectorAll<HTMLElement>('.thread-overflow-item'))
      .find(b => b.textContent?.trim() === 'Move to Current')!;
    expect(item.querySelector('svg')!.outerHTML).toBe(markup(<CurrentIcon />));
  });

  it('leaves Move to Current off the composer row: the thread menu owns it', () => {
    threadMap.value = new Map([['t', makeThreadState('t', {
      meta: { channel: 'claude_code', status: 'idle', section: 'archived' },
    })]]);
    focusedThreadId.value = 't';
    expect(getWaitingState()).toBeNull();
  });
});

describe('the Ongoing grouping button', () => {
  it('keeps the inbox tray', () => {
    setDrawerGrouping('folders');
    mount(<ThreadGroupingButton />);
    expect(host.querySelector('.crossfade-layer[data-layer="ongoing"]')!.innerHTML).toBe(markup(<InboxIcon />));
  });
});
