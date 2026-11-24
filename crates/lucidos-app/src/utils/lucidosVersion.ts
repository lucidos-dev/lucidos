export function lucidosVersionLabel(release: string | null, dirty: boolean): string {
  if (!release) return 'System';
  return dirty ? `${release} *` : release;
}

export function lucidosVersionTooltip(release: string | null, dirty: boolean): string | undefined {
  if (!release) return undefined;
  return dirty
    ? `Lucidos ${release} · code has changed since this release`
    : `Lucidos ${release}`;
}
