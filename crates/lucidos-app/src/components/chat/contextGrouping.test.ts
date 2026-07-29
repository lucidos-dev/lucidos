import { describe, it, expect } from 'vitest';
import { groupSections } from './contextGrouping';

describe('groupSections', () => {
  it('puts system-role sections in a system bucket', () => {
    const out = groupSections([
      { name: 'System Instructions', char_count: 100, role: 'system' },
      { name: 'User Profile', char_count: 50, role: 'user', group: 'Identity & profile' },
    ]);
    expect(out[0].label).toBe('System role');
    expect(out[0].innerGroups[0].sections[0].name).toBe('System Instructions');
  });

  it('orders user inner groups by TIER_ORDER', () => {
    const out = groupSections([
      { name: 'A', char_count: 1, role: 'user', group: 'Active context' },
      { name: 'B', char_count: 1, role: 'user', group: 'Identity & profile' },
    ]);
    const userTier = out.find(r => r.role === 'user')!;
    expect(userTier.innerGroups[0].name).toBe('Identity & profile');
    expect(userTier.innerGroups[1].name).toBe('Active context');
  });

  it('treats legacy sections with no role as user-role ungrouped', () => {
    const out = groupSections([{ name: 'Old', char_count: 1 }]);
    const userTier = out.find(r => r.role === 'user')!;
    expect(userTier.innerGroups[0].name).toBeNull();
    expect(userTier.innerGroups[0].sections[0].name).toBe('Old');
  });

  it('omits roles with no sections', () => {
    const out = groupSections([{ name: 'X', char_count: 1, role: 'user', group: 'Identity & profile' }]);
    expect(out.find(r => r.role === 'system')).toBeUndefined();
    expect(out.find(r => r.role === 'prior_message')).toBeUndefined();
  });

  it('appends unknown groups alphabetically after known tiers', () => {
    const out = groupSections([
      { name: 'A', char_count: 1, role: 'user', group: 'Identity & profile' },
      { name: 'X', char_count: 1, role: 'user', group: 'ZZZ Custom' },
      { name: 'Y', char_count: 1, role: 'user', group: 'AAA Custom' },
    ]);
    const userTier = out.find(r => r.role === 'user')!;
    expect(userTier.innerGroups.map(g => g.name)).toEqual(['Identity & profile', 'AAA Custom', 'ZZZ Custom']);
  });

  it('includes prior_message sections in their own bucket', () => {
    const out = groupSections([
      { name: 'ToolUse: query_events', char_count: 10, role: 'prior_message' },
      { name: 'ToolUse: load_knowhow', char_count: 20, role: 'prior_message' },
    ]);
    const prior = out.find(r => r.role === 'prior_message')!;
    expect(prior.innerGroups[0].sections.length).toBe(2);
  });
});
