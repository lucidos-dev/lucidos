import type { Bounds, EffortCost } from '../../api/types';

export interface UsageProfile {
  name: string;
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

export function monthlyCost(profile: UsageProfile, cost: EffortCost): MonthlyCost {
  const turns = profile.turnsPerMonth;
  return {
    central: turns * cost.turn_usd_central,
    range: { low: turns * cost.turn_usd.low, high: turns * cost.turn_usd.high },
  };
}
