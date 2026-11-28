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

function namesOneValue(field: string): boolean {
  return /^\d+$/.test(field);
}

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

export function describeCron(cron: string): string {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 6) return cron;

  const [second, minute, hour, dom, month, dow] = parts;

  if (!namesOneValue(second)) return cron;

  const everyDay = dom === '*' && month === '*' && dow === '*';

  if (minute === '*' && hour === '*' && everyDay) {
    return 'Every minute';
  }

  if (minute.startsWith('*/') && hour === '*' && everyDay) {
    const n = parseInt(minute.slice(2));
    if (!isNaN(n)) return n === 1 ? 'Every minute' : `Every ${n} minutes`;
  }

  if (namesOneValue(minute) && hour === '*' && everyDay) {
    const m = parseInt(minute);
    return m === 0 ? 'Every hour' : `Every hour at :${String(m).padStart(2, '0')}`;
  }

  if (hour.startsWith('*/') && namesOneValue(minute) && everyDay) {
    const n = parseInt(hour.slice(2));
    if (!isNaN(n)) return n === 1 ? 'Every hour' : `Every ${n} hours`;
  }

  const hasTime = namesOneValue(hour) && namesOneValue(minute);
  const timeStr = hasTime ? formatTime(hour, minute) : '';

  const monthStr = month !== '*' ? ` in ${MONTH_NAMES[month.toUpperCase()] || month}` : '';

  if (hasTime && dom === '*' && dow === '*' && month === '*') {
    return `Daily at ${timeStr}`;
  }

  if (hasTime && dom === '*' && dow !== '*' && month === '*') {
    const dayDesc = describeWeekday(dow);
    if (dayDesc === 'weekdays') return `Weekdays at ${timeStr}`;
    if (dayDesc === 'weekends') return `Weekends at ${timeStr}`;
    return `Every ${dayDesc} at ${timeStr}`;
  }

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

  if (hasTime && dom !== '*' && month === '*' && dow === '*') {
    const d = bareDayOfMonth(dom);
    if (d !== null) {
      return `${ordinal(d)} of every month at ${timeStr}`;
    }
  }

  if (hasTime && dom !== '*' && dow !== '*') {
    const d = bareDayOfMonth(dom);
    if (d !== null) {
      return `${ordinal(d)} & ${describeWeekday(dow)}${monthStr} at ${timeStr}`;
    }
  }

  return cron;
}

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

    if (i === 5 && /^[A-Z,-]+$/i.test(field)) continue;
    if (i === 4 && /^[A-Z,-]+$/i.test(field)) continue;

    const segments = field.split(',');
    for (const seg of segments) {
      const stepMatch = seg.match(/^(\*|\d+)\/(\d+)$/);
      if (stepMatch) {
        const step = parseInt(stepMatch[2]);
        if (step === 0 || step > maxValues[i]) return `Invalid step value in ${fieldNames[i]}: ${seg}`;
        continue;
      }
      const rangeMatch = seg.match(/^(\d+)-(\d+)$/);
      if (rangeMatch) {
        const a = parseInt(rangeMatch[1]);
        const b = parseInt(rangeMatch[2]);
        if (a > maxValues[i] || b > maxValues[i]) return `Value out of range in ${fieldNames[i]}: ${seg}`;
        continue;
      }
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
