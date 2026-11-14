import { formatDateTime, formatShortTime, isSameDayInUserTz } from './formatTime';

export function spendCapResetTime(iso: string): string {
  const date = new Date(iso);
  return isSameDayInUserTz(date, new Date()) ? formatShortTime(date) : formatDateTime(date);
}

export function spendCapResetLabel(iso: string | null | undefined): string {
  return iso ? `Resets at ${spendCapResetTime(iso)}.` : 'Reset time not supplied.';
}
