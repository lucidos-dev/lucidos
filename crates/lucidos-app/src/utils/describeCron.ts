const WEEKDAY_NAMES: Record<string, string> = {
  '0': 'Sunday', '1': 'Monday', '2': 'Tuesday', '3': 'Wednesday',
  '4': 'Thursday', '5': 'Friday', '6': 'Saturday', '7': 'Sunday',
  SUN: 'Sunday', MON: 'Monday', TUE: 'Tuesday', WED: 'Wednesday',
  THU: 'Thursday', FRI: 'Friday', SAT: 'Saturday',
};

const MONTH_NAMES: Record<string, string> = {
  '1': 'Jan', '2': 'Feb', '3': 'Mar', '4': 'Apr',
  '5': 'May', '6': 'Jun', '7': 'Jul', '8': 'Aug',
  '9': 'Sep', '10': 'Oct', '11': 'Nov', '12': 'Dec',
  JAN: 'Jan', FEB: 'Feb', MAR: 'Mar', APR: 'Apr',
  MAY: 'May', JUN: 'Jun', JUL: 'Jul', AUG: 'Aug',
  SEP: 'Sep', OCT: 'Oct', NOV: 'Nov', DEC: 'Dec',
};

function ordinal(n: number): string {
  const s = ['th', 'st', 'nd', 'rd'];
  const v = n % 100;
  return n + (s[(v - 20) % 10] || s[v] || s[0]);
}

/** Whether the field names exactly one value.
 *
 *  A step, a list (`1,15`) or a range names more than one, and `parseInt` reads
 *  only the leading token. A stepped day therefore rendered as `NaNth`, `1,15`
 *  as `1st` dropping the 15, and an hour of `9,17` as `09:00` dropping the
 *  17:00 run. Callers fall back to the raw cron rather than state a schedule
 *  the expression does not have. */
function namesOneValue(field: string): boolean {
  return /^\d+$/.test(field);
}

/** The day of the month when the field names exactly one day, else null. */
function bareDayOfMonth(dom: string): number | null {
  return namesOneValue(dom) ? parseInt(dom) : null;
}

function formatTime(hour: string, minute: string): string {
  const h = parseInt(hour);
  const m = parseInt(minute);
  if (isNaN(h) || isNaN(m)) return '';
  return m === 0
    ? `${String(h).padStart(2, '0')}:00`
    : `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

function describeWeekday(dow: string): string {
  if (dow === 'MON-FRI' || dow === '1-5') return 'weekdays';
  if (dow === 'SAT,SUN' || dow === '6,7' || dow === '0,6') return 'weekends';
  const parts = dow.split(',');
  if (parts.length > 1) {
    return parts.map(p => WEEKDAY_NAMES[p.trim().toUpperCase()] || p).join(', ');
  }
  if (dow.includes('-')) {
    const [a, b] = dow.split('-');
    const nameA = WEEKDAY_NAMES[a.trim().toUpperCase()];
    const nameB = WEEKDAY_NAMES[b.trim().toUpperCase()];
    if (nameA && nameB) return `${nameA}–${nameB}`;
  }
  return WEEKDAY_NAMES[dow.toUpperCase()] || dow;
}

/**
 * Convert a 6-field cron expression to a human-readable description.
 * Format: second minute hour day-of-month month day-of-week
 */
export function describeCron(cron: string): string {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 6) return cron;

  const [, minute, hour, dom, month, dow] = parts;

  // Every minute
  if (minute === '*' && hour === '*' && dom === '*' && dow === '*') {
    return 'Every minute';
  }

  // Every N minutes (*/N)
  if (minute.startsWith('*/') && hour === '*' && dom === '*' && dow === '*') {
    const n = parseInt(minute.slice(2));
    if (!isNaN(n)) return n === 1 ? 'Every minute' : `Every ${n} minutes`;
  }

  // Every hour at :MM
  if (namesOneValue(minute) && hour === '*' && dom === '*' && dow === '*') {
    const m = parseInt(minute);
    return m === 0 ? 'Every hour' : `Every hour at :${String(m).padStart(2, '0')}`;
  }

  // Every N hours
  if (hour.startsWith('*/') && dom === '*' && dow === '*') {
    const n = parseInt(hour.slice(2));
    if (!isNaN(n)) return n === 1 ? 'Every hour' : `Every ${n} hours`;
  }

  const hasTime = namesOneValue(hour) && namesOneValue(minute);
  const timeStr = hasTime ? formatTime(hour, minute) : '';

  // Specific month constraint
  const monthStr = month !== '*' ? ` in ${MONTH_NAMES[month.toUpperCase()] || month}` : '';

  // Daily at HH:MM
  if (hasTime && dom === '*' && dow === '*' && month === '*') {
    return `Daily at ${timeStr}`;
  }

  // Weekday pattern
  if (hasTime && dom === '*' && dow !== '*' && month === '*') {
    const dayDesc = describeWeekday(dow);
    if (dayDesc === 'weekdays') return `Weekdays at ${timeStr}`;
    if (dayDesc === 'weekends') return `Weekends at ${timeStr}`;
    return `Every ${dayDesc} at ${timeStr}`;
  }

  // Specific date: both day-of-month and month are set (not recurring)
  if (hasTime && dom !== '*' && month !== '*' && dow === '*') {
    const d = bareDayOfMonth(dom);
    const monthName = MONTH_NAMES[month.toUpperCase()] || month;
    if (d !== null) {
      const m = parseInt(month);
      const now = new Date();
      let year = now.getFullYear();
      if (!isNaN(m)) {
        const candidate = new Date(year, m - 1, d, parseInt(hour), parseInt(minute));
        if (candidate <= now) year++;
      }
      return `${monthName} ${d}, ${year} at ${timeStr}`;
    }
  }

  // Monthly on Nth day (month is *)
  if (hasTime && dom !== '*' && month === '*' && dow === '*') {
    const d = bareDayOfMonth(dom);
    if (d !== null) {
      return `${ordinal(d)} of every month at ${timeStr}`;
    }
  }

  // Specific day of month + day of week (both set)
  if (hasTime && dom !== '*' && dow !== '*') {
    const d = bareDayOfMonth(dom);
    if (d !== null) {
      return `${ordinal(d)} & ${describeWeekday(dow)}${monthStr} at ${timeStr}`;
    }
  }

  // Fallback: return the raw cron
  return cron;
}

/**
 * Basic frontend validation of a 6-field cron expression.
 * Returns null if valid, or an error message.
 */
export function validateCron(cron: string): string | null {
  const trimmed = cron.trim();
  if (!trimmed) return 'Cron expression is required';

  const parts = trimmed.split(/\s+/);
  if (parts.length !== 6) return `Expected 6 fields (sec min hour day month weekday), got ${parts.length}`;

  const fieldNames = ['second', 'minute', 'hour', 'day-of-month', 'month', 'day-of-week'];
  const maxValues = [59, 59, 23, 31, 12, 7];

  for (let i = 0; i < 6; i++) {
    const field = parts[i];
    if (field === '*') continue;

    // Allow named weekdays/months
    if (i === 5 && /^[A-Z,-]+$/i.test(field)) continue;
    if (i === 4 && /^[A-Z,-]+$/i.test(field)) continue;

    // Check each comma-separated part
    const segments = field.split(',');
    for (const seg of segments) {
      // Step value: */N or N/N
      const stepMatch = seg.match(/^(\*|\d+)\/(\d+)$/);
      if (stepMatch) {
        const step = parseInt(stepMatch[2]);
        if (step === 0 || step > maxValues[i]) return `Invalid step value in ${fieldNames[i]}: ${seg}`;
        continue;
      }
      // Range: N-N
      const rangeMatch = seg.match(/^(\d+)-(\d+)$/);
      if (rangeMatch) {
        const a = parseInt(rangeMatch[1]);
        const b = parseInt(rangeMatch[2]);
        if (a > maxValues[i] || b > maxValues[i]) return `Value out of range in ${fieldNames[i]}: ${seg}`;
        continue;
      }
      // Single number
      if (/^\d+$/.test(seg)) {
        const n = parseInt(seg);
        if (n > maxValues[i]) return `Value out of range in ${fieldNames[i]}: ${n} (max ${maxValues[i]})`;
        continue;
      }
      return `Invalid syntax in ${fieldNames[i]}: ${seg}`;
    }
  }

  return null;
}
