import type { BackfillProgress, Bounds } from '../../api/types';
import { rangeLabel } from '../../utils/rangeLabel';

const COMPACT = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });

/** `low to high`, or one figure when both read the same. */
function range(bounds: Bounds, format: (n: number) => string): string {
  const [low, high] = [format(bounds.low), format(bounds.high)];
  return low === high ? low : rangeLabel(low, high);
}

/** A call or token count, compact: `210K to 260K`. */
export function formatCount(bounds: Bounds): string {
  return range(bounds, (n) => COMPACT.format(n));
}

/** One dollar figure: cents under a dollar, whole dollars above. */
export function usd(n: number): string {
  if (n === 0) return '$0';
  if (n < 1) return `$${n.toFixed(2)}`;
  return `$${Math.round(n).toLocaleString('en')}`;
}

export function formatUsd(bounds: Bounds): string {
  return range(bounds, usd);
}

/** The central figure, with its range beside it: `$3,400 ($2,200 to $4,600)`. */
export function formatUsdCentral(central: number, bounds: Bounds): string {
  return `${usd(central)} (${formatUsd(bounds)})`;
}

/** The spend line under the bar: `$12 of $30 cap`, or the spend alone for a
 *  backfill with no cap. `null` while no backfill episode is open. */
export function formatBackfillSpend(progress: BackfillProgress): string | null {
  if (progress.spent_usd === null) return null;
  const spent = usd(progress.spent_usd);
  return progress.cap_usd === null ? `${spent} spent` : `${spent} of ${usd(progress.cap_usd)} cap`;
}

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** A duration in the unit its high bound reads best in: `6 to 19 hours`. */
export function formatDuration(secs: Bounds): string {
  if (secs.high < MINUTE) return 'under a minute';
  const [unit, name] = secs.high < 90 * MINUTE
    ? [MINUTE, 'minute']
    : secs.high < 48 * HOUR
      ? [HOUR, 'hour']
      : [DAY, 'day'];
  const at = (n: number) => String(Math.max(1, Math.round(n / unit)));
  const figure = range(secs, at);
  return `${figure} ${name}${figure === '1' ? '' : 's'}`;
}
