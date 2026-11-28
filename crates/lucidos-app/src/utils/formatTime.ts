import { preferences } from '../store/store';

function getUserTimezone(): string | undefined {
  const prefs = preferences.value;
  return prefs.status === 'loaded' ? prefs.data.timezone || undefined : undefined;
}

const formatters = new WeakMap<Intl.DateTimeFormatOptions, Map<string, Intl.DateTimeFormat>>();

function formatInUserTz(options: Intl.DateTimeFormatOptions, date: Date): string {
  if (Number.isNaN(date.getTime())) return 'Invalid Date';
  const tz = getUserTimezone();
  let byTz = formatters.get(options);
  if (!byTz) {
    byTz = new Map();
    formatters.set(options, byTz);
  }
  const key = tz ?? '';
  let formatter = byTz.get(key);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat([], tz ? { ...options, timeZone: tz } : options);
    byTz.set(key, formatter);
  }
  return formatter.format(date);
}

const DATE_TIME: Intl.DateTimeFormatOptions = {
  year: 'numeric', month: '2-digit', day: '2-digit',
  hour: '2-digit', minute: '2-digit', second: '2-digit',
  hour12: false,
};
const TIME_HM: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit', hour12: false };
const TIME_HMS: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false };
const MONTH_DAY: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };
const MONTH_DAY_YEAR: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric', year: 'numeric' };
const CALENDAR_DAY: Intl.DateTimeFormatOptions = {};

export function formatDateTime(date: Date): string {
  return formatInUserTz(DATE_TIME, date);
}

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

export function formatAgoPhrase(then: Date, now: Date): string {
  const seconds = Math.max(0, Math.floor((now.getTime() - then.getTime()) / 1000));
  if (seconds < 60) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return agoWords(minutes, 'minute');
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return agoWords(hours, 'hour');
  return agoWords(Math.floor(hours / 24), 'day');
}

function agoWords(count: number, unit: string): string {
  return `${countWords(count, unit)} ago`;
}

export function formatDurationPhrase(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 60) return 'under a minute';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return countWords(minutes, 'minute');
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return countWords(hours, 'hour');
  return countWords(Math.floor(hours / 24), 'day');
}

export function countWords(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? '' : 's'}`;
}

export function formatShortTime(date: Date): string {
  return formatInUserTz(TIME_HM, date);
}

export function formatShortDate(date: Date): string {
  return formatInUserTz(MONTH_DAY, date);
}

export function formatShortDateWithYear(date: Date): string {
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return formatInUserTz(sameYear ? MONTH_DAY : MONTH_DAY_YEAR, date);
}

export function isSameDayInUserTz(a: Date, b: Date): boolean {
  return formatInUserTz(CALENDAR_DAY, a) === formatInUserTz(CALENDAR_DAY, b);
}

export function formatNotificationDate(date: Date): string {
  const now = new Date();
  const time = formatShortTime(date);
  const isToday = isSameDayInUserTz(date, now);
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  const isYesterday = isSameDayInUserTz(date, yesterday);

  if (isToday) {
    return `Today ${time}`;
  } else if (isYesterday) {
    return `Yesterday ${time}`;
  } else {
    return formatShortDate(date) + ' ' + time;
  }
}

export function formatMessageTimestamp(isoTimestamp: string): string {
  const date = new Date(isoTimestamp);
  const time = formatInUserTz(TIME_HMS, date);
  const isToday = isSameDayInUserTz(date, new Date());

  if (isToday) {
    return `Today ${time}`;
  } else {
    return formatShortDate(date) + ' ' + time;
  }
}
