import { usd } from '../settings/treeBackfillFormat';

export interface RungSwitchCopyInput {
  /** The rung in force, as the control names it. */
  from: string;
  /** The rung the pick would commit. */
  to: string;
  /** The engine's approximate one-time cost. `null` when it has none. */
  extraCostUsd: number | null;
  /** Whether the confirm offers a new thread on the rung. The copy only
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

/** The words of the confirm a switch raises when it rebuilds a warm cache. The
 *  cost is a one-time re-read of the thread, so the copy never implies the
 *  thread runs slower afterwards. */
export function rungSwitchCopy({ from, to, extraCostUsd, canHandOff }: RungSwitchCopyInput): RungSwitchCopy {
  const rewrite = `Reads its whole history again, one time.${costClause(extraCostUsd)}`;
  return {
    rewrite,
    title: `Switch to ${to}`,
    message: canHandOff
      ? `The thread's prompt cache is warm on ${from}.`
      : `Switching to ${to} makes the thread read its whole history again, one time.${costClause(extraCostUsd)}`,
    handoffLabel: 'Hand off to a new thread',
    handoffDetail: `Starts fresh on ${to} and carries on from where this one stands.`,
    switchHereLabel: 'Switch this thread',
    keepLabel: `Keep ${from}`,
    switchLabel: 'Switch anyway',
  };
}
