// @vitest-environment jsdom
/**
 * A press on the grouping button that switches to Ongoing selects a group:
 * Blocked when the button carried a badge, else the stored group while
 * it has rows, else the first group with rows.
 * The pure rule is `ongoingGroupOnSwitch`, pinned in ongoing-groups.test.ts.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadGroupingButton } from '../ThreadsHeaderControls';
import { threadMap, drawerGrouping, setDrawerGrouping, selectedOngoingGroup, setSelectedOngoingGroup } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { ThreadMeta } from '../../../store/thread-events';

let host: HTMLElement;

function thread(id: string, meta: Partial<ThreadMeta>) {
  return [id, makeThreadState(id, { meta: { section: 'inbox', ...meta } })] as const;
}

function press() {
  act(() => { render(<ThreadGroupingButton />, host); });
  act(() => { host.querySelector<HTMLElement>('.grouping-btn')!.click(); });
}

beforeEach(() => {
  localStorage.clear();
  setDrawerGrouping('folders');
  setSelectedOngoingGroup('review');
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  threadMap.value = new Map();
  setDrawerGrouping('folders');
  setSelectedOngoingGroup('blocked');
});

describe('switching to Ongoing', () => {
  it('selects Blocked when the button carries a badge', () => {
    threadMap.value = new Map([
      thread('q', { status: 'waiting_for_user_answer' }),
      thread('r', { status: 'running' }),
    ]);
    press();
    expect(drawerGrouping.value).toBe('ongoing');
    expect(selectedOngoingGroup.value).toBe('blocked');
  });

  it('moves off an empty stored group to the first group with rows', () => {
    threadMap.value = new Map([thread('r', { status: 'running' })]);
    press();
    expect(selectedOngoingGroup.value).toBe('in-flight');
  });

  it('leaves the selected group alone on the way back to Folders', () => {
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('drafts');
    threadMap.value = new Map([thread('r', { status: 'running' })]);
    press();
    expect(drawerGrouping.value).toBe('folders');
    expect(selectedOngoingGroup.value).toBe('drafts');
  });
});
