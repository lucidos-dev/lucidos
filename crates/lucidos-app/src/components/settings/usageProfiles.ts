import type { Bounds, EffortCost } from '../../api/types';

/** A way of using a workspace, sized in turns a month, so the memory choice
 *  can say what Tree costs a month. A coding thread averages about five
 *  turns, a chat two to four (docs/plans/2026-10-14-v0-49-0-release-blockers.md). */
export interface UsageProfile {
  name: string;
  /** What a user of this profile does in a month, in their words. */
  does: string;
  turnsPerMonth: number;
}

export const USAGE_PROFILES: readonly UsageProfile[] = [
  { name: 'Light', does: 'About 10 chats a month', turnsPerMonth: 50 },
  { name: 'Medium', does: '20 coding threads and some chats a month', turnsPerMonth: 250 },
  { name: 'Heavy', does: '200 coding threads a month', turnsPerMonth: 1500 },
];

export interface MonthlyCost {
  central: number;
  range: Bounds;
}

/** A month of `profile` at one turn's price. */
export function monthlyCost(profile: UsageProfile, cost: EffortCost): MonthlyCost {
  const turns = profile.turnsPerMonth;
  return {
    central: turns * cost.turn_usd_central,
    range: { low: turns * cost.turn_usd.low, high: turns * cost.turn_usd.high },
  };
}
