// @vitest-environment jsdom
/**
 * **A click anywhere on a change row opens its thread, except on a control.**
 * A change of several commits draws a "N commits" fold, and only its toggle
 * keeps its own click. The fold's block spans the row, so a guard on the whole
 * block would leave that line of every multi-commit row dead.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { ChangesView } from '../ChangesView';
import { focusThreadOrBootstrap } from '../../../store/actions/threads';
import { changes, appliedChanges, setAsideChanges } from '../../../store/store';
import type { Change } from '../../../api/client';

function makeChange(over: Partial<Change> = {}): Change {
  return {
    id: 'change-1',
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: 'thread-1',
    thread_title: 'Thread',
    branch_name: 'b',
    repo_root: '/r',
    description: 'desc',
    file_count: 1,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: ['fix: one', 'fix: two'],
    summary: null,
    incomplete: false,
    ...over,
  };
}

const rows = {
  'a pending row': () => { changes.value = { status: 'loaded', data: [makeChange()] }; },
  'a set-aside row': () => { setAsideChanges.value = { status: 'loaded', data: [makeChange({ status: 'set_aside' })] }; },
  'an applied row': () => { appliedChanges.value = { status: 'loaded', data: [makeChange({ status: 'applied' })] }; },
};

let host: HTMLDivElement;

function click(el: Element) {
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

function q(selector: string): HTMLElement {
  const el = host.querySelector<HTMLElement>(selector);
  if (!el) throw new Error(`no ${selector}`);
  return el;
}

beforeEach(() => {
  vi.mocked(focusThreadOrBootstrap).mockClear();
  changes.value = { status: 'loaded', data: [] };
  appliedChanges.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe.each(Object.entries(rows))('%s with several commits', (_name, serve) => {
  beforeEach(async () => {
    serve();
    await act(() => { render(<ChangesView />, host); });
  });

  it('opens the thread on a click beside the fold toggle', () => {
    click(q('.change-row-commits .event-row-fold'));
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('thread-1', { targetChangeId: 'change-1' });
  });

  it('opens the thread on a click in the unfolded commit list', async () => {
    await act(() => { click(q('.event-row-fold-toggle')); });
    click(q('.change-row-commits .event-row-fold-body'));
    expect(focusThreadOrBootstrap).toHaveBeenCalledTimes(1);
  });

  it('keeps the fold toggle click to itself', async () => {
    await act(() => { click(q('.event-row-fold-toggle')); });
    expect(q('.event-row-fold-toggle').getAttribute('aria-expanded')).toBe('true');
    expect(focusThreadOrBootstrap).not.toHaveBeenCalled();
  });
});
