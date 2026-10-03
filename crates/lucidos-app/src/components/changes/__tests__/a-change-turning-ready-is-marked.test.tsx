// @vitest-environment jsdom
/**
 * **A change that turns ready while the panel is open wears the arrival
 * marker in Ready.** The panel shows its current state as it opens, so a
 * change that turned ready while it was shut is not marked.
 * Plan: `docs/plans/2026-10-02-arrival-motion-for-list-rows.md`.
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
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

const working = makeChange({ id: 'working', description: 'Working', thread_unsettled: true, thread_settling: true });
const ready = makeChange({ id: 'ready', description: 'Ready one', thread_id: 'thread-2' });

let host: HTMLDivElement;

async function serve(pending: Change[]) {
  await act(() => { changes.value = { status: 'loaded', data: pending }; });
}

/** The row showing `description`. The marker must sit on the row itself:
 *  its wash is an inset shadow, so on a wrapper the row's hover tint would
 *  paint over it. */
function rowFor(description: string): HTMLElement | null {
  const title = [...host.querySelectorAll<HTMLElement>('.change-description')].find(el => el.textContent === description);
  return title?.closest<HTMLElement>('.change-row') ?? null;
}
const isMarked = (description: string) => !!rowFor(description)?.classList.contains('arrival-marker');

beforeEach(async () => {
  appliedChanges.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  changes.value = { status: 'loaded', data: [ready, working] };
  host = document.createElement('div');
  document.body.appendChild(host);
  await act(() => { render(<ChangesView />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('a change turning ready', () => {
  it('marks nothing on the first view', () => {
    expect(rowFor('Ready one')).not.toBeNull();
    expect(isMarked('Ready one') || isMarked('Working')).toBe(false);
  });

  it('wears the arrival marker once it lands in Ready', async () => {
    await serve([ready, { ...working, thread_unsettled: false, thread_settling: false }]);
    expect(isMarked('Working')).toBe(true);
    expect(isMarked('Ready one')).toBe(false);
  });

  it('is not marked when it turned ready while the panel was shut', async () => {
    await act(() => { render(null, host); });
    await serve([ready, { ...working, thread_unsettled: false, thread_settling: false }]);
    await act(() => { render(<ChangesView />, host); });
    expect(isMarked('Working')).toBe(false);
  });
});
