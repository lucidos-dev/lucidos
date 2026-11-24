import type { BackfillProgress, Bounds } from '../../api/types';
import { rangeLabel } from '../../utils/rangeLabel';

const COMPACT = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });

function range(bounds: Bounds, format: (n: number) => string): string {
  const [low, high] = [format(bounds.low), format(bounds.high)];
  return low === high ? low : rangeLabel(low, high);
}

export function formatCount(bounds: Bounds): string {
  return range(bounds, (n) => COMPACT.format(n));
}

export function usd(n: number): string {
  if (n === 0) return '$0';
  if (n < 1) return `$${n.toFixed(2)}`;
  return `$${Math.round(n).toLocaleString('en')}`;
}

export function formatUsd(bounds: Bounds): string {
  return range(bounds, usd);
}

export function formatUsdCentral(central: number, bounds: Bounds): string {
  return `${usd(central)} (${formatUsd(bounds)})`;
}

export function formatBackfillSpend(progress: BackfillProgress): string | null {
  if (progress.spent_usd === null) return null;
  const spent = usd(progress.spent_usd);
  return progress.cap_usd === null ? `${spent} spent` : `${spent} of ${usd(progress.cap_usd)} cap`;
}

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

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
