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

    expect(appliedThreadFilter.value).toBe(before);
  });
});

describe('the surfaces behind the panel read the applied filter, never the live signals', () => {
  const LIVE_SIGNALS = ['threadChannelFilter', 'selectedTriggerIds', 'selectedRepoIds', 'selectedAppIds'];
  const SUBJECTS = [
    'components/drawer/ThreadDrawer.tsx',
    'store/actions/thread-loading.ts',
    'components/drawer/drawerOrder.ts',
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
