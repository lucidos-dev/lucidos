// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadFilterPanel } from '../ThreadFilterPanel';
import {
  ALL_CHANNELS, threadChannelFilter, threadMap, triggers,
  setSelectedTriggerIds, setSelectedRepoIds, setSelectedAppIds,
} from '../../../store/store';

let host: HTMLElement;

const mixedBoxes = () =>
  Array.from(host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')).filter(i => i.indeterminate);

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  threadMap.value = new Map();
  triggers.value = {
    status: 'loaded',
    data: [
      { id: 'trig-a', name: 'Nightly digest', run: { type: 'intent' } },
      { id: 'trig-b', name: 'Release notes', run: { type: 'intent' } },
    ] as never,
  };
  threadChannelFilter.value = new Set(ALL_CHANNELS);
  setSelectedRepoIds(new Set());
  setSelectedAppIds(new Set());
  setSelectedTriggerIds(new Set());
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

describe('ThreadFilterPanel: the channel checkbox shows a mixed state', () => {
  it('goes indeterminate on the DOM when some children of an open channel are picked', () => {
    act(() => { setSelectedTriggerIds(new Set(['trig-a'])); });
    act(() => { render(<ThreadFilterPanel />, host); });
    expect(mixedBoxes()).toHaveLength(1);
  });

  it('clears the mixed state once the children are no longer picked', () => {
    act(() => { setSelectedTriggerIds(new Set(['trig-a'])); });
    act(() => { render(<ThreadFilterPanel />, host); });
    expect(mixedBoxes()).toHaveLength(1);
    act(() => { setSelectedTriggerIds(new Set()); });
    act(() => { render(<ThreadFilterPanel />, host); });
    expect(mixedBoxes()).toHaveLength(0);
  });
});
