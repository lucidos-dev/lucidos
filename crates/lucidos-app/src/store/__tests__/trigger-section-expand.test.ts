// `expandTriggerSection` opens a collapsed section and never closes an open one.
// A trigger deep link calls it to mount a row hidden inside a collapsed section.
// A toggle here would hide the very row the link is revealing.
import { describe, it, expect, beforeEach } from 'vitest';
import {
  collapsedTriggerSectionIds,
  expandTriggerSection,
  toggleTriggerSectionCollapsed,
} from '../store';

describe('expandTriggerSection', () => {
  beforeEach(() => {
    localStorage.clear();
    collapsedTriggerSectionIds.value = new Set();
  });

  it('opens a collapsed section', () => {
    collapsedTriggerSectionIds.value = new Set(['g1', 'g2']);

    expandTriggerSection('g1');

    expect([...collapsedTriggerSectionIds.value]).toEqual(['g2']);
  });

  it('leaves an already-open section open', () => {
    collapsedTriggerSectionIds.value = new Set(['g2']);

    expandTriggerSection('g1');

    expect([...collapsedTriggerSectionIds.value]).toEqual(['g2']);
  });

  it('persists the open state, so a reload does not re-collapse', () => {
    toggleTriggerSectionCollapsed('g1');
    expect(localStorage.getItem('lucidos-collapsed-trigger-groups')).toContain('g1');

    expandTriggerSection('g1');

    expect(localStorage.getItem('lucidos-collapsed-trigger-groups')).toBe('[]');
  });
});
