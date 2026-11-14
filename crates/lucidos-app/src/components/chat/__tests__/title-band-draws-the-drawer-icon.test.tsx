// @vitest-environment jsdom
/**
 * The title band draws the same thread icon as the thread's drawer row, after
 * the status mark and before the title.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ThreadTitleMenu } from '../ThreadTitle';
import { ThreadRow } from '../../drawer/ThreadDrawer';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

vi.mock('../../../store/actions/threads', () => ({
  focusThread: vi.fn(),
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));
vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

let band: HTMLDivElement;
let row: HTMLDivElement;

function mount(home: boolean): void {
  const thread = makeThreadState('t', { meta: { title: 'Home', section: 'inbox', ...(home ? { home: true } : {}) } });
  threadMap.value = new Map([['t', thread]]);
  render(<ThreadTitleMenu thread={thread} title="Home" status="running" />, band);
  render(<ThreadRow threadId="t" status="idle" />, row);
}

beforeEach(() => {
  band = document.createElement('div');
  row = document.createElement('div');
  document.body.append(band, row);
});

afterEach(() => {
  render(null, band);
  render(null, row);
  band.remove();
  row.remove();
  threadMap.value = new Map();
});

describe('the title band', () => {
  it("draws the drawer row's icon between the status mark and the title", () => {
    mount(true);
    const icon = band.querySelector('.thread-title-text > .thread-icon');
    expect(icon?.outerHTML).toBe(row.querySelector('.thread-icon')?.outerHTML);
    expect(icon?.previousElementSibling?.classList.contains('thread-status')).toBe(true);
  });

  it('draws no icon where the drawer row draws none', () => {
    mount(false);
    expect(row.querySelector('.thread-icon')).toBeNull();
    expect(band.querySelector('.thread-icon')).toBeNull();
  });
});
