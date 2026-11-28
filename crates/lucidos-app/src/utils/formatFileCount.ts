export function formatFileCount(count: number): string {
  if (count === 0) return 'No file changes';
  return `${count} file${count === 1 ? '' : 's'}`;
}
