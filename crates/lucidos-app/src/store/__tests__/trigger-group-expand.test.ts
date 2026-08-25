// `expandTriggerGroup` opens a collapsed group and never closes an open one.
// A trigger deep link calls it to mount a row hidden inside a collapsed group.
// A toggle here would hide the very row the link is revealing.
import { describe, it, expect, beforeEach } from 'vitest';
import {
  collapsedTriggerGroupIds,
  expandTriggerGroup,
  toggleTriggerGroupCollapsed,
} from '../store';

describe('expandTriggerGroup', () => {
  beforeEach(() => {
    localStorage.clear();
    collapsedTriggerGroupIds.value = new Set();
  });

  it('opens a collapsed group', () => {
    collapsedTriggerGroupIds.value = new Set(['g1', 'g2']);

    expandTriggerGroup('g1');

    expect([...collapsedTriggerGroupIds.value]).toEqual(['g2']);
  });

  it('leaves an already-open group open', () => {
    collapsedTriggerGroupIds.value = new Set(['g2']);

    expandTriggerGroup('g1');

    expect([...collapsedTriggerGroupIds.value]).toEqual(['g2']);
  });

  it('persists the open state, so a reload does not re-collapse', () => {
    toggleTriggerGroupCollapsed('g1');
    expect(localStorage.getItem('lucidos-collapsed-trigger-groups')).toContain('g1');

    expandTriggerGroup('g1');

    expect(localStorage.getItem('lucidos-collapsed-trigger-groups')).toBe('[]');
  });
});
