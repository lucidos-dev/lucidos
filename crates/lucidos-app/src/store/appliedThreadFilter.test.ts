/**
 * The *applied thread filter*: the selection the drawer LIST renders and
 * paginates from, as opposed to the live signals the *thread filter panel*'s
 * checkboxes write.
 *
 * The panel covers the list completely (`.thread-filter-panel` is
 * `position: absolute; inset: 0` over the pane's own opaque background), so
 * every tick used to re-run the drawer's O(threads) categorization, rebuild
 * every row, and fire a page + archived-count fetch for rows nobody can see,
 * synchronously, ahead of the paint that shows the box as ticked. Holding the
 * applied selection while the panel is up is what makes a tick cost the panel's
 * own render and nothing else.
 *
 * The no-op close case has its own test because it is the one that decides
 * whether the drawer refetches: the applied value's IDENTITY is what
 * `ThreadList` memoizes and diffs on, so a close that rewrote it with an equal
 * copy would recategorize and refetch on every open/close of the panel.
 */
import { beforeEach, describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  ALL_CHANNELS,
  CODING_AGENT_CHANNEL,
  selectedAppIds,
  selectedRepoIds,
  selectedTriggerIds,
  threadChannelFilter,
  type ThreadChannel,
} from './store';
import { closeThreadFilterPanel, openThreadFilterPanel } from './threadFilterPanel';
import { appliedThreadFilter } from './appliedThreadFilter';

function everyChannel(): Set<ThreadChannel> {
  return new Set(ALL_CHANNELS);
}

describe('appliedThreadFilter', () => {
  beforeEach(() => {
    closeThreadFilterPanel();
    threadChannelFilter.value = everyChannel();
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
    selectedAppIds.value = new Set();
  });

  it('tracks the live selection while the panel is closed', () => {
    threadChannelFilter.value = new Set([CODING_AGENT_CHANNEL]);
    expect([...appliedThreadFilter.value.channels]).toEqual([CODING_AGENT_CHANNEL]);

    selectedRepoIds.value = new Set(['repo-a']);
    expect([...appliedThreadFilter.value.repoIds]).toEqual(['repo-a']);
  });

  it('holds the previous selection while the panel covers the list', () => {
    const before = appliedThreadFilter.value;
    openThreadFilterPanel();

    threadChannelFilter.value = new Set([CODING_AGENT_CHANNEL]);
    selectedRepoIds.value = new Set(['repo-a']);
    selectedAppIds.value = new Set(['habit-tracker']);
    selectedTriggerIds.value = new Set(['trig-a']);

    expect(appliedThreadFilter.value).toBe(before);
  });

  it('catches up in ONE pass when the panel closes', () => {
    openThreadFilterPanel();
    threadChannelFilter.value = new Set([CODING_AGENT_CHANNEL]);
    selectedRepoIds.value = new Set(['repo-a']);
    selectedAppIds.value = new Set(['habit-tracker']);

    const held = appliedThreadFilter.value;
    let applies = 0;
    const stop = appliedThreadFilter.subscribe(() => { applies++; });
    // `subscribe` fires once on registration with the current value.
    expect(applies).toBe(1);

    closeThreadFilterPanel();

    expect(applies).toBe(2);
    expect(appliedThreadFilter.value).not.toBe(held);
    expect([...appliedThreadFilter.value.channels]).toEqual([CODING_AGENT_CHANNEL]);
    expect([...appliedThreadFilter.value.repoIds]).toEqual(['repo-a']);
    expect([...appliedThreadFilter.value.appIds]).toEqual(['habit-tracker']);
    stop();
  });

  it('leaves the applied selection untouched when the panel closes unchanged', () => {
    const before = appliedThreadFilter.value;
    openThreadFilterPanel();
    closeThreadFilterPanel();
    expect(appliedThreadFilter.value).toBe(before);
  });

  it('holds a selection the user toggles back and forth to where it started', () => {
    threadChannelFilter.value = new Set([CODING_AGENT_CHANNEL]);
    const before = appliedThreadFilter.value;
    openThreadFilterPanel();

    threadChannelFilter.value = everyChannel();
    threadChannelFilter.value = new Set([CODING_AGENT_CHANNEL]);
    closeThreadFilterPanel();

    // Equal contents, so nothing to reapply: the drawer must not recategorize
    // or refetch for a selection that never actually moved.
    expect(appliedThreadFilter.value).toBe(before);
  });
});

/**
 * Source-scan tripwire, because this project runs Vitest with no jsdom: the
 * tests above can prove the signal holds, but not that the DRAWER is the thing
 * reading it. A single live read put back into `ThreadList` restores the whole
 * defect (that render is the synchronous work the checkbox paint waits on) and
 * would pass every test above.
 *
 * Deliberately scoped to the two files where a live read means "the list", not
 * to every consumer. The panel, its option lists and the header's
 * filter-active highlight all read the live signals on purpose.
 *
 * It forbids the NAMES rather than a particular use of them, which is broader
 * than the rule it enforces and is meant to be: everything either of these two
 * files does with the selection (render it, page it, count it, stamp what was
 * fetched against it) has to be about the one the drawer is displaying. If a
 * live read ever earns its place here, that argument belongs in this list as an
 * exception, not in a silently loosened scan.
 */
describe('the surfaces behind the panel read the applied filter, never the live signals', () => {
  const LIVE_SIGNALS = ['threadChannelFilter', 'selectedTriggerIds', 'selectedRepoIds', 'selectedAppIds'];
  const SUBJECTS = [
    // The drawer list: renders the rows and owns the refetch effect.
    'components/drawer/ThreadDrawer.tsx',
    // Pagination + the archived-count badge, which must target the same set the
    // list is showing or the cursor drifts from the rows it extends, plus the
    // stamp of what the loaded window was fetched against.
    'store/actions/thread-loading.ts',
    // The post-archive focus hand-off, which walks the drawer's visible rows to
    // pick the next one: it has to offer a row the user can actually click.
    'store/actions/threads.ts',
  ];

  const srcDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');

  for (const relative of SUBJECTS) {
    it(`${relative} reads appliedThreadFilter`, () => {
      const source: string = readFileSync(resolve(srcDir, relative), 'utf8');
      expect(source).toContain('appliedThreadFilter');
      for (const live of LIVE_SIGNALS) {
        expect(
          source.includes(live),
          `${relative} reads the live \`${live}\`. That is the panel's own state, and while the `
          + 'panel is up it covers the thread list, so the list would recategorize, rebuild every '
          + 'row and refetch for rows nobody can see, ahead of the paint that shows the ticked '
          + 'box. Read `appliedThreadFilter` instead.',
        ).toBe(false);
      }
    });
  }
});
