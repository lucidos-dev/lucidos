import type { CacheRebuild } from '../../api/client/successor';
import { usd } from '../settings/treeBackfillFormat';

export interface RungSwitchCopyInput {
  from: string;
  to: string;
  extraCostUsd: number | null;
  rebuild: CacheRebuild;
  canHandOff: boolean;
}

export interface RungSwitchCopy {
  rewrite: string;
  title: string;
  message: string;
  handoffLabel: string;
  handoffDetail: string;
  switchHereLabel: string;
  keepLabel: string;
  switchLabel: string;
}

const ONE_CENT_USD = 0.01;

function costClause(extraCostUsd: number | null): string {
  if (extraCostUsd === null) return '';
  return extraCostUsd < ONE_CENT_USD
    ? ` It costs less than ${usd(ONE_CENT_USD)} more.`
    : ` It costs about ${usd(extraCostUsd)} more.`;
}

export function rungSwitchCopy({ from, to, extraCostUsd, canHandOff, rebuild }: RungSwitchCopyInput): RungSwitchCopy {
  const may = rebuild === 'unknown';
  const cost = costClause(extraCostUsd);
  const rewrite = may
    ? `May read its whole history again, one time.${cost}`
    : `Reads its whole history again, one time.${cost}`;
  const handoffMessage = may
    ? `The thread's prompt cache on ${from} may still be warm.`
    : `The thread's prompt cache is warm on ${from}.`;
  const switchMessage = may
    ? `Switching to ${to} may make the thread read its whole history again, one time.${cost}`
    : `Switching to ${to} makes the thread read its whole history again, one time.${cost}`;
  return {
    rewrite,
    title: `Switch to ${to}`,
    message: canHandOff ? handoffMessage : switchMessage,
    handoffLabel: 'Hand off to a fresh session',
    handoffDetail: `Starts fresh on ${to} and carries on from where this one stands.`,
    switchHereLabel: 'Switch this thread',
    keepLabel: `Keep ${from}`,
    switchLabel: 'Switch anyway',
  };
}
