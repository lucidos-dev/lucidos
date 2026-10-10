import { describe, expect, it } from 'vitest';
import type { FootprintItem, WorkspacePromptFootprint } from '../../api/client';
import { LARGEST_ITEMS_SHOWN, costlySections, findingsLine, itemsFor, meterFraction, usageLine } from './promptFootprint';

function item(over: Partial<FootprintItem>): FootprintItem {
  return { kind: 'app', id: 'habit-tracker', name: 'Habit Tracker', chars: 100, clipped_chars: 0, ...over };
}

function report(over: Partial<WorkspacePromptFootprint> = {}): WorkspacePromptFootprint {
  return {
    sections: [
      { id: 'available-apps', title: 'Available Apps', when: 'every-turn', chars: 7000, over_ceiling: true, items: [
        item({ id: 'a', chars: 300, clipped_chars: 40, usage: { last_used_days_ago: 90, verdict: 'unused' } }),
        item({ id: 'b', chars: 200, usage: { last_used_days_ago: 2, verdict: 'used' } }),
      ] },
      { id: 'knowhow-routing', title: 'Know-how routing list', when: 'every-turn', chars: 900, over_ceiling: false, items: [
        item({ kind: 'knowhow', id: 'ops/nightly', name: 'Nightly', chars: 500, usage: { last_used_days_ago: null, verdict: 'not-yet-judged' } }),
      ] },
      { id: 'open-app-knowhow', title: 'Open app know-how', when: 'open-app', chars: 0, over_ceiling: false, items: [item({ id: 'a', chars: 10 })] },
    ],
    total_chars: 7900,
    over_total_ceiling: false,
    section_ceiling: 6000,
    total_ceiling: 20000,
    unused_days: 60,
    system_prompt_chars: 126000,
    system_prompt_areas: [],
    ...over,
  };
}

describe('the prompt footprint page helpers', () => {
  it('lists only sections that cost anything, largest first', () => {
    expect(costlySections(report()).map((s) => s.id)).toEqual(['available-apps', 'knowhow-routing']);
  });

  it('caps a meter at full when a section is over its ceiling', () => {
    expect(meterFraction(3000, 6000)).toBe(0.5);
    expect(meterFraction(9000, 6000)).toBe(1);
  });

  it('lists the largest items, capped, then the clipped and unused ones', () => {
    expect(itemsFor(report(), 'largest').map((i) => i.id)).toEqual(['ops/nightly', 'a', 'b', 'a']);
    expect(itemsFor(report(), 'largest').length).toBeLessThanOrEqual(LARGEST_ITEMS_SHOWN);
    expect(itemsFor(report(), 'clipped').map((i) => i.id)).toEqual(['a']);
    // The open-app section names `a` again, unjudged, so it is unused once.
    expect(itemsFor(report(), 'unused').map((i) => i.id)).toEqual(['a']);
  });

  it('leads the audit card with what the audit would find', () => {
    expect(findingsLine(report())).toBe('1 section over its ceiling, 1 clipped, 1 unused');
    expect(findingsLine(report({ over_total_ceiling: true }))).toContain('the total over its ceiling');
    const clean = report();
    clean.sections = clean.sections.map((s) => ({ ...s, over_ceiling: false, items: [] }));
    expect(findingsLine(clean)).toBe('Nothing over a ceiling, clipped or unused');
  });

  it('says when an item was last used in the verb its kind takes', () => {
    expect(usageLine(item({ usage: { last_used_days_ago: 2, verdict: 'used' } }))).toBe('opened 2 days ago');
    expect(usageLine(item({ kind: 'knowhow', usage: { last_used_days_ago: null, verdict: 'unused' } }))).toBe('never loaded');
    expect(usageLine(item({ kind: 'reusable-widget', usage: { last_used_days_ago: 0, verdict: 'used' } }))).toBe('shown today');
    expect(usageLine(item({ usage: { last_used_days_ago: null, verdict: 'not-yet-judged' } }))).toBeNull();
    expect(usageLine(item({ kind: 'knowhow', usage: { last_used_days_ago: null, verdict: 'not-loaded-by-name' } }))).toBe('not loaded by name');
  });

  it('lists a knowhow doc nothing read by name on the Unused tab', () => {
    const r = report();
    r.sections[1].items[0].usage = { last_used_days_ago: 80, verdict: 'not-loaded-by-name' };
    expect(itemsFor(r, 'unused').map((i) => i.id)).toEqual(['a', 'ops/nightly']);
    expect(findingsLine(r)).toBe('1 section over its ceiling, 1 clipped, 1 unused, 1 not loaded by name');
  });
});
