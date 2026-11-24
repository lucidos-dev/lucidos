export function changeBadgeLabel(status: string): string {
  if (status === 'modified') return 'M';
  if (status === 'added') return 'A';
  if (status === 'deleted') return 'D';
  return '?';
}
