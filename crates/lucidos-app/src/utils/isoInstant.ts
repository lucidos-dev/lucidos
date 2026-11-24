const FRACTION = /\.(\d+)(?=[Z+-]|$)/;

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
