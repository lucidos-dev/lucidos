import { preferences } from '../store/store';

/** Get the user's configured timezone from preferences, or undefined for browser default. */
function getUserTimezone(): string | undefined {
  const prefs = preferences.value;
  return prefs.status === 'loaded' ? prefs.data.timezone || undefined : undefined;
}

/** "2026-03-12 04:02:29" — full date+time in user's timezone, 24h format */
export function formatDateTime(date: Date): string {
  const tz = getUserTimezone();
  return date.toLocaleString([], {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
    hour12: false,
    ...(tz ? { timeZone: tz } : {}),
  });
}

/** How long something has been running: "8s", "2m 14s", "1h 03m".
 *
 *  A DURATION, not a point in time, so it takes milliseconds rather than a
 *  `Date` and never touches the user's timezone. Seconds are kept in the minute
 *  range because that is where the value is read as a live counter (the status
 *  toast's build timer ticks once a second, and a counter that only changed each
 *  minute would read as frozen); past an hour they are noise, so the hour form
 *  zero-pads minutes instead and the string stops changing every second.
 *
 *  A negative or non-finite input clamps to "0s": the caller derives this from
 *  clock arithmetic, and a "-3s" build age is worse than a momentarily stalled
 *  one. */
export function formatElapsed(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '0s';
  const totalSeconds = Math.floor(ms / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

/** "just now", "5m ago", "3h ago", "2d ago", or short date */
export function formatTimeAgo(date: Date): string {
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;
  return formatShortDate(date);
}

/** "14:30" — short HH:MM time in user's timezone */
export function formatShortTime(date: Date): string {
  const tz = getUserTimezone();
  return date.toLocaleTimeString([], {
    hour: '2-digit', minute: '2-digit',
    hour12: false,
    ...(tz ? { timeZone: tz } : {}),
  });
}

/** "Feb 28" — short month + day in user's timezone */
export function formatShortDate(date: Date): string {
  const tz = getUserTimezone();
  return date.toLocaleDateString([], {
    month: 'short', day: 'numeric',
    ...(tz ? { timeZone: tz } : {}),
  });
}

/** "Feb 28" for current year, "Feb 28, 2025" for past years. */
export function formatShortDateWithYear(date: Date): string {
  const tz = getUserTimezone();
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString([], {
    month: 'short', day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
    ...(tz ? { timeZone: tz } : {}),
  });
}

/** "Today 14:30", "Yesterday 14:30", or "Feb 28 14:30" */
export function formatNotificationDate(date: Date): string {
  const now = new Date();
  const tz = getUserTimezone();
  const time = formatShortTime(date);
  // Compare dates in the user's configured timezone, not the browser's local tz.
  const dateOpts = tz ? { timeZone: tz } : {};
  const dateStr = date.toLocaleDateString([], dateOpts);
  const isToday = dateStr === now.toLocaleDateString([], dateOpts);
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = dateStr === yesterday.toLocaleDateString([], dateOpts);

  if (isToday) {
    return `Today ${time}`;
  } else if (isYesterday) {
    return `Yesterday ${time}`;
  } else {
    return formatShortDate(date) + ' ' + time;
  }
}

/** "Today 14:30:05" or "Feb 28 14:30:05" — includes seconds */
export function formatMessageTimestamp(isoTimestamp: string): string {
  const date = new Date(isoTimestamp);
  const now = new Date();
  const tz = getUserTimezone();
  const time = date.toLocaleTimeString([], {
    hour: '2-digit', minute: '2-digit', second: '2-digit',
    hour12: false,
    ...(tz ? { timeZone: tz } : {}),
  });
  // Compare dates in the user's configured timezone, not the browser's local tz.
  // toDateString() uses browser tz which causes "Today" to flicker when they differ.
  const dateOpts = tz ? { timeZone: tz } : {};
  const isToday = date.toLocaleDateString([], dateOpts) === now.toLocaleDateString([], dateOpts);

  if (isToday) {
    return `Today ${time}`;
  } else {
    return formatShortDate(date) + ' ' + time;
  }
}
