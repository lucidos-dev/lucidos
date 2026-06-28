import { describe, it, expect, beforeEach } from 'vitest';
import {
  triggers,
  repositories,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
  threadChannelFilter,
  ALL_CHANNELS,
} from './store';
import { threadFilterActive } from './threadFilterActive';
import { makeTrigger } from './__tests__/fixtures';

describe('threadFilterActive', () => {
  beforeEach(() => {
    triggers.value = { status: 'not-loaded' };
    repositories.value = { status: 'not-loaded' };
    selectedTriggerIds.value = new Set();
    selectedRepoIds.value = new Set();
    selectedAppIds.value = new Set();
    threadChannelFilter.value = new Set(ALL_CHANNELS);
  });

  it('is false when every channel is on and no per-trigger / per-repo subset is set', () => {
    expect(threadFilterActive.value).toBe(false);
  });

  it('is true when a channel is turned off', () => {
    const next = new Set(threadChannelFilter.value);
    next.delete('chat');
    threadChannelFilter.value = next;
    expect(threadFilterActive.value).toBe(true);
  });

  it('is true when the trigger channel is on but only a subset of triggers is selected', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'a', name: 'A' }),
        makeTrigger({ id: 'b', name: 'B' }),
      ],
    };
    selectedTriggerIds.value = new Set(['a']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is false when the trigger channel is on with every available trigger explicitly selected', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'a', name: 'A' }),
        makeTrigger({ id: 'b', name: 'B' }),
      ],
    };
    selectedTriggerIds.value = new Set(['a', 'b']);
    expect(threadFilterActive.value).toBe(false);
  });

  it('is false when the trigger channel is on with no per-trigger selection (= all triggers)', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'a', name: 'A' }),
        makeTrigger({ id: 'b', name: 'B' }),
      ],
    };
    selectedTriggerIds.value = new Set();
    expect(threadFilterActive.value).toBe(false);
  });

  it('is true when triggers selection is restored from localStorage before the registry loads', () => {
    triggers.value = { status: 'loading' };
    selectedTriggerIds.value = new Set(['a']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is true when repos selection is restored from localStorage before the registry loads', () => {
    repositories.value = { status: 'loading' };
    selectedRepoIds.value = new Set(['r1']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is false when only one trigger exists (lockstep — subset and all coincide)', () => {
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'only', name: 'Only' })] };
    selectedTriggerIds.value = new Set(['only']);
    expect(threadFilterActive.value).toBe(false);
  });

  it('is true when the coding-agent channel is on but only a subset of repos is selected', () => {
    repositories.value = {
      status: 'loaded',
      data: [
        { id: 'r1', name: 'Repo 1', path: '/r1' },
        { id: 'r2', name: 'Repo 2', path: '/r2' },
      ],
    };
    selectedRepoIds.value = new Set(['r1']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is true even with every repo selected — app coding-agent threads are still excluded because the filter is two-axis', () => {
    // Coding Agent has two cross-axis sub-selections (repos + apps).
    // threadPassesChannelFilter unions repoOk || appOk under the
    // coding-agent branch, so picking "every repo" still drops every app coding-agent
    // thread from the drawer — the filter IS narrowing. The per-axis
    // "selected === total" shortcut from triggers doesn't apply.
    repositories.value = {
      status: 'loaded',
      data: [
        { id: 'r1', name: 'Repo 1', path: '/r1' },
        { id: 'r2', name: 'Repo 2', path: '/r2' },
      ],
    };
    selectedRepoIds.value = new Set(['r1', 'r2']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is true when the coding-agent channel is on and any app is selected', () => {
    selectedAppIds.value = new Set(['habit-tracker']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is true when both a repo and an app are selected under Coding Agent', () => {
    selectedRepoIds.value = new Set(['r1']);
    selectedAppIds.value = new Set(['habit-tracker']);
    expect(threadFilterActive.value).toBe(true);
  });

  it('is false when the coding-agent channel is on with no per-target selection (= all coding-agent threads)', () => {
    expect(threadFilterActive.value).toBe(false);
  });

  it('ignores selectedTriggerIds when the trigger channel itself is off (the channel-off flag already wins)', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'a', name: 'A' }),
        makeTrigger({ id: 'b', name: 'B' }),
      ],
    };
    selectedTriggerIds.value = new Set(['a']);
    const next = new Set(threadChannelFilter.value);
    next.delete('trigger');
    threadChannelFilter.value = next;
    expect(threadFilterActive.value).toBe(true);
  });
});
