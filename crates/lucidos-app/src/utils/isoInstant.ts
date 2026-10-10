/** Sub-second digits, and whatever ends them: `Z`, an offset sign, or nothing. */
const FRACTION = /\.(\d+)(?=[Z+-]|$)/;

/** The instant an ISO-8601 timestamp names, in epoch microseconds.
 *
 *  Use this to ORDER two server timestamps. Comparing the strings is wrong.
 *  The engine writes only the digits it needs, so an event on a whole second
 *  arrives as `...:21Z` while its neighbours carry `...:21.010Z`. `.` sorts
 *  before `Z`, so a lexical compare puts the whole second LAST. Frontend
 *  stamps always carry 3 digits, so widths differ across producers too.
 *
 *  Microseconds because that is what Postgres keeps, and the count stays an
 *  exact integer well past the year 2200. The fraction is read here rather
 *  than handed to `Date.parse`, which is specified for 3 digits only and
 *  truncates the rest. So `Date.parse` sees a whole second and nothing else.
 *
 *  `null` for an absent or unparsable value, so a caller decides what an
 *  unorderable timestamp means rather than reading a silent epoch 0. */
export function instantMicros(iso: string | undefined | null): number | null {
  if (!iso) return null;
  const fraction = FRACTION.exec(iso);
  const whole = fraction
    ? iso.slice(0, fraction.index) + iso.slice(fraction.index + fraction[0].length)
    : iso;
  const ms = Date.parse(whole);
  if (Number.isNaN(ms)) return null;
  const micros = fraction ? Number(fraction[1].padEnd(6, '0').slice(0, 6)) : 0;
  return ms * 1000 + micros;
}
