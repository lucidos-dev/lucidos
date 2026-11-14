import type { CacheRebuild } from '../../api/client/successor';
import { usd } from '../settings/treeBackfillFormat';

export interface RungSwitchCopyInput {
  /** The rung in force, as the control names it. */
  from: string;
  /** The rung the pick would commit. */
  to: string;
  /** The engine's approximate one-time cost. `null` when it has none. */
  extraCostUsd: number | null;
  /** `unknown` when the cache may already be cold: the copy then says the
   *  switch *may* reread the history. */
  rebuild: CacheRebuild;
  /** Whether the confirm offers a fresh session on the rung. The copy only
   *  advertises a handoff it offers. */
  canHandOff: boolean;
}

export interface RungSwitchCopy {
  /** The cost of switching this thread, without naming the rung. */
  rewrite: string;
  /** The title, naming the target rung. Shown with the confirm choices. */
  title: string;
  message: string;
  handoffLabel: string;
  handoffDetail: string;
  switchHereLabel: string;
  keepLabel: string;
  /** The OK when no handoff is offered. */
  switchLabel: string;
}

/** A cent is the least `usd` prints without rounding a figure to nothing. */
const ONE_CENT_USD = 0.01;

/** The engine prices the switch, so this only words it. */
function costClause(extraCostUsd: number | null): string {
  if (extraCostUsd === null) return '';
  return extraCostUsd < ONE_CENT_USD
    ? ` It costs less than ${usd(ONE_CENT_USD)} more.`
    : ` It costs about ${usd(extraCostUsd)} more.`;
}

/** The words of the confirm a switch raises when it rebuilds a warm cache, or
 *  may. The cost is a one-time re-read of the thread, so the copy never
 *  implies the thread runs slower afterwards. */
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
