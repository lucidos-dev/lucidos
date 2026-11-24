export function formatBytes(bytes: number, round: (value: number) => number = Math.round): string {
  if (bytes < 1024) return `${round(bytes)} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unitIdx = 0;
  while (value >= 1024 && unitIdx < units.length - 1) {
    value /= 1024;
    unitIdx += 1;
  }
  const scale = unitIdx >= 2 ? 10 : 1;
  return `${(round(value * scale) / scale).toFixed(unitIdx >= 2 ? 1 : 0)} ${units[unitIdx]}`;
}
